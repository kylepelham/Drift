use tokio::io::AsyncWriteExt;

use super::*;

impl Snapshots {
    pub(super) async fn git(&self, workspace: &Path, args: &[&str]) -> Result<String, Error> {
        let bytes = self.git_bytes(workspace, args).await?;

        Ok(String::from_utf8_lossy(&bytes).trim().to_string())
    }

    pub(super) async fn git_bytes(&self, workspace: &Path, args: &[&str]) -> Result<Vec<u8>, Error> {
        self.run(workspace, args, None).await
    }

    pub(super) async fn run(&self, workspace: &Path, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, Error> {
        self.run_with(workspace, args, input, None).await
    }

    /// Runs shadow-repository git with a source repository's objects available for tree commands.
    pub(super) async fn run_with(
        &self,
        workspace: &Path,
        args: &[&str],
        input: Option<&[u8]>,
        source: Option<&Source>,
    ) -> Result<Vec<u8>, Error> {
        let mut command = self.command(workspace, args, input.is_some());
        if let Some(source) = source {
            command.env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &source.objects);
        }
        let mut child = command.spawn().map_err(|_| Error::NoGit)?;
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            stdin
                .write_all(input)
                .await
                .map_err(|error| Error::Failed(error.to_string()))?;
        }

        let output = child
            .wait_with_output()
            .await
            .map_err(|error| Error::Failed(error.to_string()))?;
        if !output.status.success() {
            return Err(Error::Failed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }

        Ok(output.stdout)
    }

    pub(super) fn command(&self, workspace: &Path, args: &[&str], piped: bool) -> Command {
        let mut command = Command::new("git");
        // Git pathspecs resolve against the working directory, not --work-tree.
        if workspace.is_dir() {
            command.current_dir(workspace);
        }
        command
            .arg("--git-dir")
            .arg(self.git_dir(workspace))
            .arg("--work-tree")
            .arg(workspace)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(if piped { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);

        command
    }

    /// Creates the shadow repository once; concurrent first captures wait for the one that creates it.
    pub(super) async fn ensure(&self, workspace: &Path) -> Result<(), Error> {
        let git_dir = self.git_dir(workspace);
        if git_dir.join("HEAD").exists() {
            return Ok(());
        }

        let lock = self.lock_for(workspace);
        let _held = lock.lock().await;
        if git_dir.join("HEAD").exists() {
            return Ok(());
        }

        tokio::fs::create_dir_all(&git_dir)
            .await
            .map_err(|error| Error::Failed(error.to_string()))?;
        self.git(workspace, &["init", "-q"]).await?;
        self.git(workspace, &["config", "core.autocrlf", "false"]).await?;
        self.git(workspace, &["config", "user.email", "drift@localhost"])
            .await?;
        self.git(workspace, &["config", "user.name", "Drift"]).await?;

        Ok(())
    }
}

/// Git in the workspace's own repository, never the shadow one; `None` when it fails.
pub(super) async fn plain_git(workspace: &Path, args: &[&str], input: Option<&[u8]>) -> Option<Vec<u8>> {
    let mut command = Command::new("git");
    let stdin = if input.is_some() { Stdio::piped() } else { Stdio::null() };
    command
        .current_dir(workspace)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = command.spawn().ok()?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(input).await.ok()?;
    }

    let output = child.wait_with_output().await.ok()?;
    output.status.success().then_some(output.stdout)
}
