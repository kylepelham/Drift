use super::*;
use crate::tool::tool_search::ToolSearch;
use std::collections::{BTreeMap, BTreeSet};

impl Steering {
    pub(super) fn of(plan: &Plan) -> Self {
        Self {
            model: plan.model_ref.clone(),
            agent: plan.session.agent.clone(),
            config: plan.config.clone(),
            workspace: plan.workspace.clone(),
            catalog: plan.catalog.clone(),
            mcp_commands: plan.mcp_commands.clone(),
            turn_only: plan.turn_only,
        }
    }

    /// The model and agent a steered prompt runs as when it names neither: the session's own during a command's turn.
    pub(super) fn defaults(&self, session: Option<&Session>) -> (ModelRef, String) {
        match session.filter(|_| self.turn_only) {
            Some(session) => (
                session.model.clone().unwrap_or_else(|| self.model.clone()),
                session.agent.clone(),
            ),
            None => (self.model.clone(), self.agent.clone()),
        }
    }
}

impl Plan {
    /// A tool this turn offers, as it was offered.
    pub(in crate::session) fn offered(&self, name: &str) -> Option<Arc<dyn crate::tool::Tool>> {
        self.offer.tool(name)
    }

    /// What the variant asks of the model now planned; a name this model does not offer asks nothing.
    pub(super) fn reasoning(&self) -> Option<Reasoning> {
        reasoning_in(&self.model.variants, self.variant.as_deref())
    }
}

impl Engine {
    pub(in crate::session) async fn plan(&self, session_id: &str, prompt: &Prompt) -> Result<Plan, TurnError> {
        self.plan_for(session_id, prompt, false, None).await
    }

    pub(super) async fn plan_for(
        &self,
        session_id: &str,
        prompt: &Prompt,
        turn_only: bool,
        config: Option<&Config>,
    ) -> Result<Plan, TurnError> {
        let mut session = self.store.session(session_id)?.ok_or(TurnError::NoSession)?;
        let workspace = self
            .store
            .workspace(&session.workspace_id)?
            .ok_or(TurnError::NoWorkspace)?;
        self.bind_permissions(&session.id, &session.workspace_id);
        self.permissions.load_auto_accept(&session.id, session.auto_accept);

        let root = crate::tool::canonical(Path::new(&workspace.path));
        let (workspace_path, owner) = self.working_place(&session, &workspace);
        let config = config.cloned().unwrap_or_else(|| self.workspace_config(&root));
        if let Some(problem) = config.problems.first() {
            return Err(TurnError::Config(problem.clone()));
        }
        if let Some(agent) = &prompt.agent {
            pickable(&config, agent)?;
            session.agent = agent.clone();
        }
        if let Some(agent) = config.agent(&session.agent) {
            agent.usable().map_err(|error| TurnError::Config(error.to_string()))?;
        }

        let agent_model = config.agent(&session.agent).and_then(|agent| agent.model.clone());
        let model_ref = prompt
            .model
            .clone()
            .or_else(|| session.model.clone())
            .or(agent_model)
            .or_else(|| config.model.clone())
            .ok_or(TurnError::NoModel)?;
        let catalog = Arc::new(self.catalog_view());
        let resolved = self.resolve_from(&model_ref, &catalog).await?;
        let provider = resolved
            .provider
            .with_timeouts(config.route_timeouts(&resolved.model_ref.provider));
        let variant = prompt
            .variant
            .clone()
            .unwrap_or_else(|| session.variant.clone())
            .or_else(|| config.agent(&session.agent).and_then(|agent| agent.variant.clone()));

        let mut plan = Plan {
            session,
            workspace: workspace_path,
            root,
            owner,
            config: Arc::new(config),
            catalog,
            model_ref: resolved.model_ref,
            model: resolved.model,
            provider,
            credential: resolved.credential,
            account: resolved.account,
            variant,
            offer: Offer::default(),
            mcp_tools: Vec::new(),
            mcp_servers: Vec::new(),
            mcp_commands: Vec::new(),
            bootstrap: Vec::new(),
            turn_only,
        };
        self.stay_where_warm(&mut plan).await;
        self.load_plan_tools(&mut plan).await;
        plan.offer = self.offer(&plan);

        Ok(plan)
    }

    async fn load_plan_tools(&self, plan: &mut Plan) {
        // Wait for workspace servers so a connecting server's tools are not omitted from this turn.
        if let Some(engine) = self.me.upgrade() {
            engine.start_workspace_mcp(&plan.root);
        }
        self.mcp.wait_ready(crate::mcp::READY_WAIT).await;
        self.mcp.refresh_stale(&self.store, &self.hub).await;

        plan.mcp_tools = self
            .mcp
            .tools(&self.store, Some(&plan.root))
            .into_iter()
            .map(|tool| (tool.spec(), tool))
            .collect();
        plan.mcp_servers = self.mcp.instructions(Some(&plan.root));
        plan.mcp_commands = self.mcp.prompt_commands(Some(&plan.root));
    }

    pub(super) async fn refresh_plan(&self, plan: &mut Plan) -> Result<(), TurnError> {
        let session = self.store.session(&plan.session.id)?.ok_or(TurnError::NoSession)?;
        if session.workspace_id != plan.session.workspace_id {
            return Err(TurnError::Moved);
        }

        let provider = plan.model_ref.provider.clone();
        let (environment, api) = plan
            .catalog
            .providers
            .get(&provider)
            .map(|provider| (provider.env.clone(), provider.api.clone()))
            .unwrap_or_default();
        let (account, stored) = self
            .credentials
            .resolve_account(&provider, &environment, |key| {
                self.account_suits(key, &plan.session.id, &plan.model)
            })
            .ok_or(TurnError::NoCredentials)?;
        oneshot::refuse_signin_elsewhere(&provider, &stored, api.as_deref())?;

        plan.credential = self.fresh_credential(&provider, account.as_deref(), stored).await?;
        plan.account = account;
        Ok(())
    }

    /// Expired subscription tokens are refreshed before they are sent.
    pub(in crate::session) async fn fresh_credential(
        &self,
        provider: &str,
        account: Option<&str>,
        credential: Credential,
    ) -> Result<Credential, TurnError> {
        if !credential.is_expired() {
            return Ok(credential);
        }

        self.renew(provider, account, credential).await
    }

    /// The provider's first account's credential for callers outside a turn, such as the shell's usage limits.
    /// An expired sign-in is renewed first; `None` when there is no credential or renewal fails.
    pub async fn current_credential(&self, provider: &str) -> Option<Credential> {
        let first = self.credentials.accounts(provider).into_iter().next()?;

        self.account_credential(provider, &first.key).await
    }

    /// Each of the provider's accounts in the user's order with its credential, renewed first when expired, for
    /// callers outside a turn. The credential is `None` when the account has none or its renewal failed.
    pub async fn account_credentials(&self, provider: &str) -> Vec<(llm::credentials::Account, Option<Credential>)> {
        let accounts = self.credentials.accounts(provider);
        let renewals = accounts
            .iter()
            .map(|account| self.account_credential(provider, &account.key));
        let credentials = futures_util::future::join_all(renewals).await;

        accounts.into_iter().zip(credentials).collect()
    }

    /// A new token for a sign-in that expired or was refused, refreshed once however many turns ask at the same time.
    pub(in crate::session) async fn renew(
        &self,
        provider: &str,
        account: Option<&str>,
        credential: Credential,
    ) -> Result<Credential, TurnError> {
        let Some(account) = account else {
            return Ok(credential);
        };
        let lock = self.turns.refresh_lock(account);
        let _held = lock.lock().await;

        // A concurrent refresh or sign-in may already have supplied a usable replacement.
        if let Some(stored) = self
            .credentials
            .account(account)
            .filter(|stored| *stored != credential && !stored.is_expired())
        {
            return Ok(stored);
        }
        let Credential::OAuth { refresh, .. } = &credential else {
            return Ok(credential);
        };

        let refreshed = match provider {
            "anthropic" => llm::anthropic::oauth::refresh(&self.http, refresh).await,
            "openai" => llm::openai::oauth::refresh(&self.http, refresh).await,
            "xai" => llm::xai::refresh(&self.http, refresh).await,
            _ => return Ok(credential),
        };
        let fresh = refreshed.map_err(|error| TurnError::SignInExpired(error.to_string()))?;

        // A sign-in or sign-out made during refresh takes precedence over the refreshed token.
        if !self
            .credentials
            .replace_if(account, &credential, &fresh)
            .map_err(|error| TurnError::Store(error.to_string()))?
        {
            return self.credentials.account(account).ok_or(TurnError::NoCredentials);
        }

        Ok(fresh)
    }

    pub(in crate::session) fn provider_for(&self, id: &str, catalog_api: Option<&str>) -> Option<Provider> {
        if let Some(provider) = self.turns.provider_override.lock().unwrap().clone() {
            return Some(provider);
        }

        llm::provider_for(id, catalog_api)
    }

    pub(crate) fn command_config(&self, session_id: &str, workspace: &Path) -> Config {
        let running = self.turns.steering.lock().unwrap().get(session_id).cloned();
        let (mut config, commands) = match running {
            Some(running) => ((*running.config).clone(), running.mcp_commands),
            None => (
                self.workspace_config(workspace),
                self.mcp.prompt_commands(Some(workspace)),
            ),
        };
        config.commands.extend(commands);

        config
    }

    /// Before each request, a prompt's chosen model, agent or reasoning level becomes the turn's.
    /// The choice is read from the session, where admission saved it.
    pub(super) async fn follow_session(&self, plan: &mut Plan) -> Result<(), FollowError> {
        // A command's temporary choices end only after the user steers in another prompt.
        if plan.turn_only {
            let newest = self.store.newest_prompt(&plan.session.id).ok().flatten();
            if newest.is_none() || newest == self.turns.began(&plan.session.id) {
                return Ok(());
            }

            plan.turn_only = false;
            if let Some(running) = self.turns.steering.lock().unwrap().get_mut(&plan.session.id) {
                running.turn_only = false;
            }
        }

        let Ok(Some(session)) = self.store.session(&plan.session.id) else {
            return Ok(());
        };
        let moved = self.follow_move(plan, &session);

        let model = session.model.clone().filter(|model| *model != plan.model_ref);
        let variant = session.variant.clone().or_else(|| {
            plan.config
                .agent(&session.agent)
                .and_then(|agent| agent.variant.clone())
        });
        if !moved && model.is_none() && session.agent == plan.session.agent && variant == plan.variant {
            return Ok(());
        }
        if session.agent != plan.session.agent {
            pickable(&plan.config, &session.agent)?;
        }

        if let Some(model) = model {
            let resolved = self
                .resolve_from(&model, &plan.catalog)
                .await
                .map_err(|error| FollowError::Switch {
                    model: model.model.clone(),
                    error,
                })?;
            plan.model_ref = resolved.model_ref;
            plan.model = resolved.model;
            plan.provider = resolved
                .provider
                .with_timeouts(plan.config.route_timeouts(&plan.model_ref.provider));
            plan.credential = resolved.credential;
            plan.account = resolved.account;
        }

        plan.session.agent = session.agent;
        plan.variant = variant;
        plan.offer = self.offer(plan);

        if let Some(running) = self.turns.steering.lock().unwrap().get_mut(&plan.session.id) {
            *running = Steering::of(plan);
        }

        Ok(())
    }

    /// A move `session_move` made takes effect here, between steps, never during a call: the next step's calls
    /// run in the new folder. True when the session moved.
    fn follow_move(&self, plan: &mut Plan, session: &Session) -> bool {
        if session.worktree == plan.session.worktree {
            return false;
        }
        let Ok(Some(workspace)) = self.store.workspace(&session.workspace_id) else {
            return false;
        };

        (plan.workspace, plan.owner) = self.working_place(session, &workspace);
        plan.session.worktree.clone_from(&session.worktree);
        true
    }

    /// Built-in tools that a setting can take away: `process` with background processes, `browser` with its switch,
    /// `websearch` with search, `generate_image` until image generation is on, and the worktree tools outside a git
    /// repository.
    fn built_in_available(&self, name: &str, repository: bool) -> bool {
        match name {
            "process" => self.background_processes(),
            "browser" => self.browser_settings().enabled,
            "websearch" => self.web_search_available(),
            "generate_image" => self.image_generation_available(),
            name if crate::worktree::TOOLS.contains(&name) => repository,
            _ => true,
        }
    }

    /// The tools and system prompt for the plan's model and agent.
    /// Only offered tools may run; calls to any other tool are refused before permission or snapshot.
    pub(super) fn offer(&self, plan: &Plan) -> Offer {
        let agent = plan.config.agent(&plan.session.agent).cloned();
        let subagent = plan.session.visibility == Visibility::Hidden;
        let rules = self
            .permissions
            .compiled(&plan.config.policy(), &plan.config.agent_policy(&plan.session.agent));
        let repository = crate::config::in_repository(&plan.root);
        let mut tools: Vec<_> = self
            .tools
            .offered(plan.model.profile)
            .into_iter()
            .map(|tool| (tool.spec_for(self), tool))
            .filter(|(spec, _)| self.built_in_available(&spec.name, repository))
            .chain(plan.mcp_tools.iter().cloned())
            .filter(|(spec, _)| agent.as_ref().is_none_or(|agent| agent.allows_tool(&spec.name)))
            .filter(|(spec, _)| !(subagent && crate::tool::task::DELEGATION.contains(&spec.name.as_str())))
            .filter(|(_, tool)| !tool.denied_outright(&rules))
            .collect();
        let deferred = self.deferred_tools(plan, &tools);
        let listed = deferred_listing(&tools, &deferred);
        if !deferred.is_empty() {
            let search: Arc<dyn crate::tool::Tool> = Arc::new(ToolSearch::new(searchable(&tools, &deferred)));
            tools.push((search.spec(), search));
        }

        // Server instructions are included only when this turn offers one of that server's tools.
        let servers: Vec<_> = plan
            .mcp_servers
            .iter()
            .filter(|(server, _)| tools.iter().any(|(_, tool)| tool.server() == Some(server.as_str())))
            .cloned()
            .collect();
        let base = prompt::base_for(&self.store, plan.model.prompt);
        let denied = |kind: &str, name: &str| {
            rules.explicit(&crate::tool::Ask::new(kind, name, "")) == Some(crate::permission::Decision::Deny)
        };
        let references = self.references_for_turn(&plan.config.references);
        let plans = self.plans_dir(&plan.session.workspace_id);
        let setting = prompt::Setting {
            base: &base,
            family: plan.model.prompt,
            workspace: &plan.workspace,
            config: &plan.config,
            agent: agent.as_ref(),
            delegates: tools.iter().any(|(spec, _)| spec.name == "task"),
            loads_skills: tools.iter().any(|(spec, _)| spec.name == "skill"),
            denied: &denied,
            model: &plan.model.name,
            servers: &servers,
            references: &references,
            plans: Some(&plans),
            deferred: &listed,
        };
        let sources = prompt::sources(&setting);

        Offer {
            tools,
            deferred,
            sources,
        }
    }

    /// The MCP tools sent for `tool_search` to load instead of up front: every tool of a server large enough that
    /// its definitions would weigh on every request, on a model whose wire takes deferred tools with the setting on.
    fn deferred_tools(&self, plan: &Plan, tools: &[OfferedTool]) -> BTreeSet<String> {
        if !plan.model.tool_search || !self.tool_search() {
            return BTreeSet::new();
        }

        let mut servers: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for (spec, tool) in tools {
            if let Some(server) = tool.server() {
                let (count, bytes) = servers.entry(server).or_default();
                *count += 1;
                *bytes += spec.name.len() + spec.description.len() + spec.parameters().to_string().len();
            }
        }
        let large = |server: &str| {
            servers
                .get(server)
                .is_some_and(|(count, bytes)| *count >= DEFER_TOOLS || *bytes >= DEFER_BYTES)
        };

        tools
            .iter()
            .filter(|(_, tool)| tool.server().is_some_and(large))
            .map(|(spec, _)| spec.name.clone())
            .collect()
    }
}

type OfferedTool = (llm::ToolSpec, Arc<dyn crate::tool::Tool>);

/// MCP servers with this many tools, or this many bytes of definitions, are loaded on request.
const DEFER_TOOLS: usize = 10;
const DEFER_BYTES: usize = 10 * 1024;

fn searchable(tools: &[OfferedTool], deferred: &BTreeSet<String>) -> Vec<(llm::ToolSpec, String)> {
    tools
        .iter()
        .filter(|(spec, _)| deferred.contains(&spec.name))
        .map(|(spec, tool)| (spec.clone(), tool.server().unwrap_or_default().to_string()))
        .collect()
}

/// The deferred tools' names by server, for the system prompt.
fn deferred_listing(tools: &[OfferedTool], deferred: &BTreeSet<String>) -> Vec<(String, Vec<String>)> {
    let mut listed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (spec, tool) in tools.iter().filter(|(spec, _)| deferred.contains(&spec.name)) {
        let server = tool.server().unwrap_or_default().to_string();
        listed.entry(server).or_default().push(spec.name.clone());
    }

    listed.into_iter().collect()
}

impl Offer {
    /// The tools in the model's context from the start.
    pub(super) fn specs(&self) -> Vec<llm::ToolSpec> {
        self.tools
            .iter()
            .filter(|(spec, _)| !self.deferred.contains(&spec.name))
            .map(|(spec, _)| spec.clone())
            .collect()
    }

    /// The tools sent for `tool_search` to load.
    pub(super) fn deferred_specs(&self) -> Vec<llm::ToolSpec> {
        self.tools
            .iter()
            .filter(|(spec, _)| self.deferred.contains(&spec.name))
            .map(|(spec, _)| spec.clone())
            .collect()
    }

    pub(super) fn tool(&self, name: &str) -> Option<Arc<dyn crate::tool::Tool>> {
        self.tools
            .iter()
            .find(|(spec, _)| spec.name == name)
            .map(|(_, tool)| tool.clone())
    }

    /// The tool by its exact name, else the one offered tool whose name differs only in case.
    pub(super) fn tool_named(&self, name: &str) -> Option<(String, Arc<dyn crate::tool::Tool>)> {
        if let Some(tool) = self.tool(name) {
            return Some((name.to_string(), tool));
        }

        let mut close = self
            .tools
            .iter()
            .filter(|(spec, _)| spec.name.eq_ignore_ascii_case(name));
        match (close.next(), close.next()) {
            (Some((spec, tool)), None) => Some((spec.name.clone(), tool.clone())),
            _ => None,
        }
    }
}

/// What a variant name asks of a model offering `variants`; a name it does not offer asks nothing.
fn reasoning_in(variants: &[Variant], name: Option<&str>) -> Option<Reasoning> {
    let name = name?;

    variants
        .iter()
        .find(|variant| variant.name == name)
        .map(|variant| variant.reasoning.clone())
}

/// Checks that the requested agent is a usable workspace primary agent that can run a conversation.
pub(super) fn pickable(config: &Config, agent: &str) -> Result<(), TurnError> {
    match config.agent(agent) {
        Some(found) if found.kind.runs_conversations() => found
            .usable()
            .map(|_| ())
            .map_err(|error| TurnError::Config(error.to_string())),
        _ => Err(TurnError::UnknownAgent),
    }
}
