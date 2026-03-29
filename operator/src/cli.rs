use anyhow::{Result, anyhow};
use std::process::Command;
use tracing::info;

pub(crate) fn rc_exec(args: Vec<&str>) -> Result<String, anyhow::Error> {
    info!("Executing rc + {:?}", args);
    let expect = format!("command has failed: rc {:?}", args);
    let output = Command::new("rc").args(args).output().expect(&expect);
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !&output.status.success() {
        return Err(anyhow!(stderr));
    };
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub(crate) fn cli_exec_from_dir(command: String, dir: String) -> Result<String, anyhow::Error> {
    info!("executing: {}", command);
    let expect = format!("command has failed: {}", command);
    let output = Command::new("sh")
        .arg("-c")
        .current_dir(dir)
        .arg(command)
        .output()
        .expect(&expect);
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !&output.status.success() {
        return Err(anyhow!(stderr));
    };
    let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
    stdout.pop();
    Ok(stdout)
}
