//! Signing in with a credential the user stored: the engine types it, so the password never reaches the model or
//! the transcript.

use serde_json::{Value, json};

use super::super::Context;
use super::hosts::host;
use crate::browser::driver::{Language, Tab};
use crate::browser::project::{Secret, fill_in};

/// Sets one field by `#id`, `name` or CSS selector and tells the page it changed; written for IE's old engines too.
const SET_FIELD: &str = "(function(s,v){var d=document,e=null;\
if(s.charAt(0)=='#')e=d.getElementById(s.substr(1));\
if(!e&&d.querySelector){try{e=d.querySelector(s)}catch(x){}}\
if(!e){var n=d.getElementsByName(s);if(n.length)e=n[0]}\
if(!e)return 'missing';if(e.focus)e.focus();e.value=v;\
if(d.createEvent){var a=d.createEvent('HTMLEvents');a.initEvent('input',true,false);e.dispatchEvent(a);\
var b=d.createEvent('HTMLEvents');b.initEvent('change',true,false);e.dispatchEvent(b)}\
else if(e.fireEvent){e.fireEvent('onchange')}return 'ok'})";

const CLICK: &str = "(function(s){var d=document,e=null;\
if(s.charAt(0)=='#')e=d.getElementById(s.substr(1));\
if(!e&&d.querySelector){try{e=d.querySelector(s)}catch(x){}}\
if(!e){var n=d.getElementsByName(s);if(n.length)e=n[0]}\
if(!e)return 'missing';e.click();return 'ok'})";

/// The conversation's workspace credential: the one named, or the project's sign-in one.
pub(super) fn secret(context: &Context, name: Option<&str>) -> Result<Secret, String> {
    let (workspace, project) = context.engine.browser_project(&context.session_id);
    let workspace = workspace.ok_or("this conversation has no workspace to keep credentials for")?;
    let name = name
        .map(str::to_string)
        .or_else(|| project.login.map(|login| login.credential))
        .ok_or("name a `credential`; this project's drift.json sets no browser.login.credential")?;

    context.engine.browser_secret(&workspace.id, &name)
}

/// Signs in as the project's `browser.login` says: an address that signs in, or a form to fill and send.
pub(super) async fn run(context: &Context, tab: &dyn Tab, input: &Value) -> Result<String, String> {
    let (_, project) = context.engine.browser_project(&context.session_id);
    let login = project
        .login
        .clone()
        .ok_or("this project has no sign-in: add `browser.login` to its drift.json (docs/browser.md shows how)")?;
    let secret = secret(context, input["credential"].as_str())?;

    if let Some(url) = &login.url {
        let address = project.resolve(&fill_in(url, &secret));
        if host(&address) != host(&project.resolve(url)) {
            return Err("browser.login.url must name its host itself, not take it from the credential".into());
        }
        tab.go(&address).await?;
        return Ok(format!("Signed in as {} through the sign-in address.", secret.user));
    }

    if let Some(path) = &login.path {
        tab.go(&project.resolve(path)).await?;
    }
    for (selector, template) in &login.fields {
        let call = format!(
            "{SET_FIELD}({}, {})",
            json!(selector),
            json!(fill_in(template, &secret))
        );
        let set = tab.evaluate(&call, Language::JavaScript).await?;
        if set == "missing" {
            return Err(format!(
                "the sign-in form has no field `{selector}`; check browser.login.fields"
            ));
        }
    }
    if let Some(submit) = &login.submit {
        let clicked = tab
            .evaluate(&format!("{CLICK}({})", json!(submit)), Language::JavaScript)
            .await?;
        if clicked == "missing" {
            return Err(format!(
                "the sign-in form has no button `{submit}`; check browser.login.submit"
            ));
        }
    }

    Ok(format!("Filled the sign-in form as {} and sent it.", secret.user))
}
