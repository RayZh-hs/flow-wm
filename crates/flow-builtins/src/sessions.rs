//! User and service-manager environments. Compositor availability is supplied by the WM.
use anyhow::{Context, Result};
use flow_core::{domain::*, providers::SessionProvider};

pub struct LocalSessions {
    definitions: Vec<WorkspaceDefinition>,
    user_incarnation: String,
    systemd_incarnation: Option<String>,
}
impl LocalSessions {
    pub fn new(
        host: HostId,
        user_incarnation: String,
        systemd_invocation: Option<String>,
    ) -> Result<Self> {
        let mut state = RuntimeState::local();
        state.bind_host(host)?;
        let definitions = state
            .workspaces
            .definitions()
            .filter(|w| w.id.0.ends_with(":user") || w.id.0.ends_with(":systemd"))
            .cloned()
            .collect();
        Ok(Self {
            definitions,
            user_incarnation,
            systemd_incarnation: systemd_invocation
                .filter(|id| id.len() == 32 && id.bytes().all(|c| c.is_ascii_hexdigit())),
        })
    }
    pub fn activate(&self, registry: &mut WorkspaceRegistry) -> Result<()> {
        for definition in self.definitions() {
            registry.register(definition.clone())?;
            let available = self.availability(&definition.id)?;
            if available.active {
                registry.activate(&definition.id, &definition.provider, available.incarnation)?;
            } else if let Some(previous) = registry.availability(&definition.id).cloned() {
                registry.end(&definition.id, &previous.incarnation)?;
            }
        }
        Ok(())
    }
}
impl SessionProvider for LocalSessions {
    fn definitions(&self) -> Vec<WorkspaceDefinition> {
        self.definitions.clone()
    }
    fn availability(&self, workspace: &WorkspaceId) -> Result<Availability> {
        let definition = self
            .definitions()
            .into_iter()
            .find(|d| &d.id == workspace)
            .context("Unknown local environment")?;
        let incarnation = if definition.provider.0 == "systemd" {
            self.systemd_incarnation.clone()
        } else {
            Some(self.user_incarnation.clone())
        };
        Ok(Availability {
            active: incarnation.is_some(),
            incarnation: incarnation.unwrap_or_default(),
        })
    }
}
