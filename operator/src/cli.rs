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

#[cfg(test)]
mod tests {
    use crate::cli::{cli_exec_from_dir, rc_exec};
    use tempfile::TempDir;

    #[test]
    fn test_stderr() {
        let command = ">&2 echo \"error\" && exit 1";
        let test = rc_exec(command.to_string());
        assert_eq!(test.err().unwrap().to_string(), "error\n".to_string());
    }

    #[test]
    fn test_stdout() {
        let command = "echo test";
        let test = rc_exec(command.to_string());
        assert_eq!(test.unwrap().to_string(), "test\n".to_string());
    }

    #[test]
    fn test_stdout_current_dir() {
        let dir = TempDir::new().unwrap();
        let dir_str = dir.path().to_str().unwrap().to_string();
        let command = "echo $PWD";
        let test = cli_exec_from_dir(command.to_string(), dir_str.clone());
        assert!(test.unwrap().to_string().contains(dir_str.as_str()));
    }

    #[test]
    fn test_stderr_current_dir() {
        let dir = TempDir::new().unwrap();
        let dir_str = dir.path().to_str().unwrap().to_string();
        let command = ">&2 echo \"error\" && exit 1";
        let test = cli_exec_from_dir(command.to_string(), dir_str.clone());
        assert_eq!(test.err().unwrap().to_string(), "error\n".to_string());
    }
}
