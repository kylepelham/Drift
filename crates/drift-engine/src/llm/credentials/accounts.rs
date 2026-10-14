//! A provider's credentials, one per account, in the order they are used.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{CredentialError, Credentials, INDEX};
use crate::llm::Credential;

/// Where the account lists are kept, beside the provider index.
pub(super) const ACCOUNTS: &str = "__accounts";

/// One credential of a provider; `key` names where its secret is stored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Who signed in, so signing in as them again replaces this account rather than adding one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
}

/// Who a sign-in belongs to, as far as the provider says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Profile {
    pub identity: Option<String>,
    pub email: Option<String>,
}

impl Account {
    pub(super) fn at(key: &str) -> Self {
        Self {
            key: key.into(),
            label: None,
            identity: None,
        }
    }

    fn signed_in(key: String, profile: &Profile) -> Self {
        Self {
            key,
            label: profile.email.clone(),
            identity: profile.identity.clone(),
        }
    }
}

impl Profile {
    /// The person a JWT access token names: the ChatGPT account and user, or the subject, and an email when it has one.
    pub fn from_jwt(token: &str) -> Self {
        let Some(claims) = claims(token) else {
            return Self::default();
        };

        let auth = &claims["https://api.openai.com/auth"];
        let user = [&auth["chatgpt_user_id"], &auth["user_id"], &claims["sub"]]
            .into_iter()
            .find_map(Value::as_str);
        let identity = match (auth["chatgpt_account_id"].as_str(), user) {
            (Some(account), Some(user)) => Some(format!("{account}/{user}")),
            (_, user) => user.map(str::to_string),
        };
        let email = [&claims["https://api.openai.com/profile"]["email"], &claims["email"]]
            .into_iter()
            .find_map(Value::as_str)
            .map(str::to_string);

        Self { identity, email }
    }
}

fn claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;

    serde_json::from_slice(&bytes).ok()
}

impl Credentials {
    /// A provider's accounts in the order they are used. A credential saved before accounts existed is its only one.
    pub fn accounts(&self, provider: &str) -> Vec<Account> {
        if let Some(listed) = self.accounts.lock().unwrap().get(provider) {
            return listed.clone();
        }

        let indexed = self.index.lock().unwrap().contains(provider);
        if indexed || self.read(provider).is_some() {
            vec![Account::at(provider)]
        } else {
            Vec::new()
        }
    }

    /// The credential one account holds.
    pub fn account(&self, key: &str) -> Option<Credential> {
        self.read(key).and_then(|json| serde_json::from_str(&json).ok())
    }

    /// Stores a sign-in and returns its account's key. Signing in as someone already listed replaces their account;
    /// anyone else joins the end of the list. An API key, or a sign-in beside one, replaces them all: only sign-ins take turns.
    pub fn add_account(
        &self,
        provider: &str,
        credential: &Credential,
        profile: &Profile,
    ) -> Result<String, CredentialError> {
        let _held = self.write_lock.lock().unwrap();
        let mut accounts = self.accounts(provider);

        // The same person signing in again keeps their place in the list.
        if let Some(same) = self.signed_in_as(&accounts, profile) {
            let account = &mut accounts[same];
            self.write(&account.key, &serde_json::to_string(credential).unwrap())?;
            account.identity.clone_from(&profile.identity);
            account.label = account.label.take().or_else(|| profile.email.clone());

            let key = account.key.clone();
            self.save_accounts(provider, accounts)?;
            return Ok(key);
        }

        if accounts.is_empty() || !self.takes_turns(credential, &accounts) {
            self.replace_all(provider, credential, Account::signed_in(provider.into(), profile))?;
            return Ok(provider.into());
        }

        let key = format!("{provider}~{}", crate::random_hex(3));
        self.write(&key, &serde_json::to_string(credential).unwrap())?;
        accounts.push(Account::signed_in(key.clone(), profile));
        self.save_accounts(provider, accounts)?;

        Ok(key)
    }

    fn signed_in_as(&self, accounts: &[Account], profile: &Profile) -> Option<usize> {
        let identity = profile.identity.as_ref()?;

        accounts
            .iter()
            .position(|account| self.identity(account).as_ref() == Some(identity))
    }

    /// Signs one account out; the others keep their order.
    pub fn remove_account(&self, provider: &str, key: &str) -> Result<bool, CredentialError> {
        let _held = self.write_lock.lock().unwrap();
        let mut accounts = self.accounts(provider);
        let Some(at) = accounts.iter().position(|account| account.key == key) else {
            return Ok(false);
        };

        self.delete(key)?;
        accounts.remove(at);
        self.save_accounts(provider, accounts)?;

        Ok(true)
    }

    /// Puts the accounts in the order `keys` names; `false` unless it names each of them once.
    pub fn reorder_accounts(&self, provider: &str, keys: &[String]) -> Result<bool, CredentialError> {
        let _held = self.write_lock.lock().unwrap();
        let mut accounts = self.accounts(provider);
        let mut sorted = keys.to_vec();
        sorted.sort();
        let mut current: Vec<String> = accounts.iter().map(|account| account.key.clone()).collect();
        current.sort();
        if sorted != current {
            return Ok(false);
        }

        accounts.sort_by_key(|account| keys.iter().position(|key| *key == account.key));
        self.save_accounts(provider, accounts)?;

        Ok(true)
    }

    /// Names an account; an empty label clears the name.
    pub fn rename_account(&self, provider: &str, key: &str, label: &str) -> Result<bool, CredentialError> {
        let _held = self.write_lock.lock().unwrap();
        let mut accounts = self.accounts(provider);
        let Some(account) = accounts.iter_mut().find(|account| account.key == key) else {
            return Ok(false);
        };

        let label = label.trim();
        account.label = (!label.is_empty()).then(|| label.to_string());
        self.save_accounts(provider, accounts)?;

        Ok(true)
    }

    /// Leaves `credential` as the provider's only account. The write lock must be held.
    pub(super) fn replace_all(
        &self,
        provider: &str,
        credential: &Credential,
        account: Account,
    ) -> Result<(), CredentialError> {
        for old in self.accounts(provider) {
            if old.key != account.key {
                self.delete(&old.key)?;
            }
        }

        self.write(&account.key, &serde_json::to_string(credential).unwrap())?;
        self.save_accounts(provider, vec![account])
    }

    /// Records the provider's accounts, and whether it has any, where they persist.
    pub(super) fn save_accounts(&self, provider: &str, accounts: Vec<Account>) -> Result<(), CredentialError> {
        let (index, listed) = {
            let mut index = self.index.lock().unwrap();
            let mut listed = self.accounts.lock().unwrap();
            if accounts.is_empty() {
                index.remove(provider);
                listed.remove(provider);
            } else {
                index.insert(provider.into());
                listed.insert(provider.into(), accounts);
            }

            (
                serde_json::to_string(&*index).unwrap(),
                serde_json::to_string(&*listed).unwrap(),
            )
        };

        self.write(INDEX, &index)?;
        self.write(ACCOUNTS, &listed)
    }

    /// Only sign-ins take turns; a list holding an API key is replaced, not added to.
    fn takes_turns(&self, credential: &Credential, accounts: &[Account]) -> bool {
        let signed_in = |credential: Option<&Credential>| matches!(credential, Some(Credential::OAuth { .. }));

        signed_in(Some(credential))
            && accounts
                .iter()
                .all(|account| signed_in(self.account(&account.key).as_ref()))
    }

    /// Who an account is: as recorded at sign-in, else as its token says for one stored before accounts existed.
    fn identity(&self, account: &Account) -> Option<String> {
        account.identity.clone().or_else(|| match self.account(&account.key)? {
            Credential::OAuth { access, .. } => Profile::from_jwt(&access).identity,
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests;
