//! Optional local Office-to-PDF conversion through a separately installed LibreOffice.
//! The caller explicitly opts into an external process; this is not a filesystem sandbox.

/// Native Open supports these formats when LibreOffice is installed.
pub const EXTENSIONS: [&str; 10] = ["docx", "doc", "pptx", "ppt", "xlsx", "xls", "odt", "odp", "ods", "rtf"];
pub const MAX_INPUT_BYTES: u64 = 128 * 1024 * 1024;

pub fn is_office(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| EXTENSIONS.iter().any(|allowed| ext.eq_ignore_ascii_case(allowed)))
}

/// Convert a copied input in a private temporary directory. No input or destination is overwritten.
/// The result still needs to be opened/validated by the engine before it is offered to the user.
pub fn convert_to_pdf(name: &str, bytes: &[u8]) -> Result<Vec<u8>, String> {
    if !is_office(name) {
        return Err("choose a Word, PowerPoint, Excel, OpenDocument or RTF file".into());
    }
    if bytes.is_empty() || bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err("Office conversion needs a nonempty file of at most 128 MiB".into());
    }
    let extension = name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).unwrap_or_default();
    let signature_matches = match extension.as_str() {
        "doc" | "ppt" | "xls" => bytes.starts_with(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]),
        "rtf" => bytes.starts_with(b"{\\rtf"),
        _ => bytes.starts_with(b"PK\x03\x04"),
    };
    if !signature_matches {
        return Err("the file content does not match its Office extension".into());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        native::convert(name, bytes)
    }
    #[cfg(target_arch = "wasm32")]
    Err("Office conversion requires the desktop app and LibreOffice".into())
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    const MAX_OUTPUT_BYTES: u64 = 256 * 1024 * 1024;
    const TIMEOUT: Duration = Duration::from_secs(120);

    // A fresh profile, no trusted macro locations, no macro execution or automatic link updates.
    // These preferences reduce document side effects; they are not an OS security boundary.
    const PROFILE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<oor:items xmlns:oor="http://openoffice.org/2001/registry">
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop><prop oor:name="DisableMacrosExecution" oor:op="fuse"><value>true</value></prop><prop oor:name="DisableActiveContent" oor:op="fuse"><value>true</value></prop><prop oor:name="BlockUntrustedRefererLinks" oor:op="fuse"><value>true</value></prop></item>
<item oor:path="/org.openoffice.Office.Writer/Content/Update"><prop oor:name="Link" oor:op="fuse"><value>2</value></prop></item>
<item oor:path="/org.openoffice.Office.Calc/Content/Update"><prop oor:name="Link" oor:op="fuse"><value>1</value></prop></item>
</oor:items>"#;

    fn executable() -> Result<PathBuf, String> {
        if let Some(path) = std::env::var_os("PDFCRAFT_LIBREOFFICE") {
            let path = PathBuf::from(path);
            if path.is_absolute() && path.is_file() {
                return Ok(path);
            }
            return Err("PDFCRAFT_LIBREOFFICE must be the absolute path to an installed LibreOffice executable".into());
        }
        let mut candidates = Vec::new();
        #[cfg(windows)]
        for key in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(dir) = std::env::var_os(key) {
                candidates.push(PathBuf::from(dir).join("LibreOffice/program/soffice.com"));
            }
        }
        #[cfg(target_os = "macos")]
        candidates.push(PathBuf::from("/Applications/LibreOffice.app/Contents/MacOS/soffice"));
        #[cfg(not(windows))]
        candidates.extend(["/usr/bin/libreoffice", "/usr/bin/soffice"].map(PathBuf::from));
        candidates.into_iter().find(|p| p.is_absolute() && p.is_file()).ok_or_else(|| {
            "Install LibreOffice to convert Office files to PDF, then try again. For a custom installation, set PDFCRAFT_LIBREOFFICE to its executable's absolute path.".into()
        })
    }

    struct Running(Child);

    impl Drop for Running {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(), Ok(Some(_))) {
                #[cfg(unix)]
                {
                    // Each conversion owns a separate process group, including launcher children.
                    let _ = Command::new("/bin/kill")
                        .args(["-KILL", "--", &format!("-{}", self.0.id())])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    if let Some(dir) = std::env::var_os("SystemRoot") {
                        let _ = Command::new(PathBuf::from(dir).join("System32/taskkill.exe"))
                            .args(["/PID", &self.0.id().to_string(), "/T", "/F"])
                            .creation_flags(0x08000000)
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status();
                    }
                }
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }

    pub(super) fn convert(name: &str, bytes: &[u8]) -> Result<Vec<u8>, String> {
        let executable = executable()?;
        convert_with(&executable, name, bytes, TIMEOUT)
    }

    fn convert_with(executable: &Path, name: &str, bytes: &[u8], timeout: Duration) -> Result<Vec<u8>, String> {
        let (_, extension) = name.rsplit_once('.').ok_or("missing Office file extension")?;
        let temporary = tempfile::Builder::new().prefix("pdfcraft-office-").tempdir().map_err(|e| e.to_string())?;
        let directory = temporary.path().canonicalize().map_err(|e| e.to_string())?;
        let user = directory.join("profile/user");
        std::fs::create_dir_all(&user).map_err(|e| e.to_string())?;
        std::fs::write(user.join("registrymodifications.xcu"), PROFILE).map_err(|e| e.to_string())?;
        let input = directory.join(format!("input.{}", extension.to_ascii_lowercase()));
        std::fs::write(&input, bytes).map_err(|e| e.to_string())?;
        let profile = url::Url::from_directory_path(directory.join("profile")).map_err(|_| "invalid temporary profile path")?;
        let mut command = Command::new(executable);
        command
            .arg(format!("-env:UserInstallation={profile}"))
            .args(["--headless", "--nologo", "--nodefault", "--norestore", "--convert-to", "pdf", "--outdir"])
            .arg(&directory)
            .arg(&input)
            .current_dir(&directory)
            .env("TMPDIR", &directory)
            .env("TMP", &directory)
            .env("TEMP", &directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = Running(command.spawn().map_err(|e| format!("could not start LibreOffice: {e}"))?);
        let start = Instant::now();
        loop {
            if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())? {
                if !status.success() {
                    return Err(format!("LibreOffice conversion failed ({status}); the original file was not changed"));
                }
                break;
            }
            if start.elapsed() >= timeout {
                return Err("LibreOffice conversion timed out; the original file was not changed".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let path = directory.join("input.pdf");
        let file =
            std::fs::File::open(&path).map_err(|_| "LibreOffice produced no PDF. Check that the document is valid and not password protected.")?;
        if file.metadata().map_err(|e| e.to_string())?.len() > MAX_OUTPUT_BYTES {
            return Err("the converted PDF exceeds the 256 MiB limit".into());
        }
        let mut output = Vec::new();
        file.take(MAX_OUTPUT_BYTES + 1).read_to_end(&mut output).map_err(|e| e.to_string())?;
        if output.len() as u64 > MAX_OUTPUT_BYTES || !output.starts_with(b"%PDF-") {
            return Err("LibreOffice did not produce a valid PDF within the size limit".into());
        }
        Ok(output)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn missing_executable_returns_error() {
            let dir = tempfile::tempdir().unwrap();
            let error = convert_with(&dir.path().join("missing"), "test.docx", b"test", Duration::from_secs(1)).unwrap_err();
            assert!(error.contains("could not start LibreOffice"));
        }

        #[cfg(unix)]
        #[test]
        fn success_without_output_is_not_conversion_success() {
            let error = convert_with(Path::new("/bin/true"), "test.docx", b"test", Duration::from_secs(1)).unwrap_err();
            assert!(error.contains("produced no PDF"));
        }

        #[cfg(unix)]
        #[test]
        fn stalled_conversion_is_stopped_at_the_deadline() {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let executable = dir.path().join("stalled-converter");
            std::fs::write(&executable, "#!/bin/sh\nsleep 30\n").unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            let start = Instant::now();
            let error = convert_with(&executable, "test.docx", b"test", Duration::from_millis(20)).unwrap_err();
            assert!(error.contains("timed out"));
            assert!(start.elapsed() < Duration::from_secs(5));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn office_extensions_and_input_limits() {
        for extension in EXTENSIONS {
            assert!(is_office(&format!("name.{}", extension.to_uppercase())));
        }
        for name in ["docx", "file.pdf", "file.docm", "file.exe", "file.docx.exe"] {
            assert!(!is_office(name));
        }
        assert!(convert_to_pdf("empty.docx", b"").unwrap_err().contains("nonempty"));
        assert!(convert_to_pdf("script.exe", b"x").is_err());
        assert!(convert_to_pdf("fake.docx", b"plain text").unwrap_err().contains("does not match"));
    }
}
