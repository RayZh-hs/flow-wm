//! Native window operations are separate from owned execution interruption.
use anyhow::{Result, ensure};
use flow_core::providers::{ApplicationOperations, ApplicationProvider};
pub struct DesktopApplications;
fn window(handle: &str) -> Result<u64> {
    Ok(handle.parse()?)
}
impl ApplicationProvider for DesktopApplications {
    fn operations(&self, handle: &str) -> Result<ApplicationOperations> {
        window(handle)?;
        Ok(ApplicationOperations {
            open: true,
            close_window: true,
            interrupt: false,
            disown: true,
        })
    }
    fn open(&self, handle: &str) -> Result<()> {
        crate::niri::focus(window(handle)?)
    }
    fn close_window(&self, handle: &str) -> Result<()> {
        crate::niri::request(niri_ipc::Request::Action(niri_ipc::Action::CloseWindow {
            id: Some(window(handle)?),
        }))?;
        Ok(())
    }
    fn interrupt(&self, _handle: &str) -> Result<()> {
        anyhow::bail!(
            "This application has not declared process-safe interruption; close its window separately"
        )
    }
    fn disown(&self, handle: &str) -> Result<()> {
        ensure!(
            self.operations(handle)?.disown,
            "Attachment cannot be disowned"
        );
        Ok(())
    }
}
