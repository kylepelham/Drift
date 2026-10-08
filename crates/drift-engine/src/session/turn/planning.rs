use super::*;

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

        let workspace_path = crate::tool::canonical(Path::new(&workspace.path));
        let config = config
            .cloned()
            .unwrap_or_else(|| self.workspace_config(&workspace_path));
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
            config: Arc::new(config),
            catalog,
            model_ref: resolved.model_ref,
            model: resolved.model,
            provider,
            credential: resolved.credential,
            variant,
            offer: Offer::default(),
            mcp_tools: Vec::new(),
            mcp_servers: Vec::new(),
            mcp_commands: Vec::new(),
            bootstrap: Vec::new(),
            turn_only,
        };
        self.load_plan_tools(&mut plan).await;
        plan.offer = self.offer(&plan);

        Ok(plan)
    }

    async fn load_plan_tools(&self, plan: &mut Plan) {
        // Wait for workspace servers so a connecting server's tools are not omitted from this turn.
        if let Some(engine) = self.me.upgrade() {
            engine.start_workspace_mcp(&plan.workspace);
        }
        self.mcp.wait_ready(crate::mcp::READY_WAIT).await;
        self.mcp.refresh_stale(&self.store, &self.hub).await;

        plan.mcp_tools = self
            .mcp
            .tools(&self.store, Some(&plan.workspace))
            .into_iter()
            .map(|tool| (tool.spec(), tool))
            .collect();
        plan.mcp_servers = self.mcp.instructions(Some(&plan.workspace));
        plan.mcp_commands = self.mcp.prompt_commands(Some(&plan.workspace));
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
        let stored = self
            .credentials
            .resolve(&provider, &environment)
            .ok_or(TurnError::NoCredentials)?;
        oneshot::refuse_signin_elsewhere(&provider, &stored, api.as_deref())?;

        plan.credential = self.fresh_credential(&provider, stored).await?;
        Ok(())
    }

    /// Expired subscription tokens are refreshed before they are sent.
    pub(in crate::session) async fn fresh_credential(
        &self,
        provider: &str,
        credential: Credential,
    ) -> Result<Credential, TurnError> {
        if !credential.is_expired() {
            return Ok(credential);
        }

        self.renew(provider, credential).await
    }

    /// The provider's stored credential, renewed first when it is a sign-in past its expiry, for
    /// callers outside a turn (the shell's usage limits); `None` when there is none or it cannot be renewed.
    pub async fn current_credential(&self, provider: &str) -> Option<Credential> {
        let stored = self.credentials.get(provider)?;
        if !stored.is_expired() {
            return Some(stored);
        }

        self.renew(provider, stored).await.ok()
    }

    /// A new token for a sign-in that expired or was refused, refreshed once however many turns ask at the same time.
    pub(in crate::session) async fn renew(
        &self,
        provider: &str,
        credential: Credential,
    ) -> Result<Credential, TurnError> {
        let lock = self.turns.refresh_lock(provider);
        let _held = lock.lock().await;

        // A concurrent refresh or sign-in may already have supplied a usable replacement.
        if let Some(stored) = self
            .credentials
            .get(provider)
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
            .replace_if(provider, &credential, &fresh)
            .map_err(|error| TurnError::Store(error.to_string()))?
        {
            return self.credentials.get(provider).ok_or(TurnError::NoCredentials);
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
        let model = session.model.clone().filter(|model| *model != plan.model_ref);
        let variant = session.variant.clone().or_else(|| {
            plan.config
                .agent(&session.agent)
                .and_then(|agent| agent.variant.clone())
        });
        if model.is_none() && session.agent == plan.session.agent && variant == plan.variant {
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
        }
        plan.session.agent = session.agent;
        plan.variant = variant;
        plan.offer = self.offer(plan);
        if let Some(running) = self.turns.steering.lock().unwrap().get_mut(&plan.session.id) {
            *running = Steering::of(plan);
        }

        Ok(())
    }

    /// The tools and system prompt for the plan's model and agent. What was offered is what may run:
    /// a call to any other tool is refused before permission or snapshot.
    pub(super) fn offer(&self, plan: &Plan) -> Offer {
        let agent = plan.config.agent(&plan.session.agent).cloned();
        let subagent = plan.session.visibility == Visibility::Hidden;
        let rules = self
            .permissions
            .compiled(&plan.config.policy(), &plan.config.agent_policy(&plan.session.agent));
        let tools: Vec<_> = self
            .tools
            .offered(plan.model.profile)
            .into_iter()
            .map(|tool| (tool.spec(), tool))
            .chain(plan.mcp_tools.iter().cloned())
            .filter(|(spec, _)| agent.as_ref().is_none_or(|agent| agent.allows_tool(&spec.name)))
            .filter(|(spec, _)| !(subagent && crate::tool::task::DELEGATION.contains(&spec.name.as_str())))
            .filter(|(_, tool)| !tool.denied_outright(&rules))
            .collect();

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
        let setting = prompt::Setting {
            base: &base,
            workspace: &plan.workspace,
            config: &plan.config,
            agent: agent.as_ref(),
            delegates: tools.iter().any(|(spec, _)| spec.name == "task"),
            loads_skills: tools.iter().any(|(spec, _)| spec.name == "skill"),
            denied: &denied,
            model: &plan.model.name,
            servers: &servers,
        };
        let system = prompt::system(&setting);

        Offer { tools, system }
    }
}

impl Offer {
    pub(super) fn specs(&self) -> Vec<llm::ToolSpec> {
        self.tools.iter().map(|(spec, _)| spec.clone()).collect()
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

/// Whether a prompt may switch its session to `agent`: only a usable primary agent of the workspace runs a conversation.
pub(super) fn pickable(config: &Config, agent: &str) -> Result<(), TurnError> {
    match config.agent(agent) {
        Some(found) if found.kind.runs_conversations() => found
            .usable()
            .map(|_| ())
            .map_err(|error| TurnError::Config(error.to_string())),
        _ => Err(TurnError::UnknownAgent),
    }
}
