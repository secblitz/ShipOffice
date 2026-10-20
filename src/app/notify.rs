//! Desktop notifications, through what each system already has: `notify-send` on Linux, the
//! notification centre through `osascript` on macOS, a toast through PowerShell on Windows.
//! Sent from a thread, so a slow or missing notifier never holds up a frame.

pub fn send(title: &str, body: &str) {
    let (title, body) = (clean(title, 80), clean(body, 200));
    std::thread::spawn(move || {
        let _ = run(&title, &body);
    });
}

/// One line, no control characters, at most `n` characters.
fn clean(s: &str, n: usize) -> String {
    let one: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let one = one.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > n {
        one.chars().take(n - 1).chain(['…']).collect()
    } else {
        one
    }
}

#[cfg(target_os = "linux")]
fn run(title: &str, body: &str) -> std::io::Result<std::process::ExitStatus> {
    std::process::Command::new("notify-send").args(["--app-name=ShipOffice", "--icon=shipoffice", title, body]).status()
}

#[cfg(target_os = "macos")]
fn run(title: &str, body: &str) -> std::io::Result<std::process::ExitStatus> {
    // AppleScript strings: backslashes and quotes escaped
    let q = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("display notification \"{}\" with title \"{}\"", q(body), q(title));
    std::process::Command::new("osascript").args(["-e", &script]).status()
}

#[cfg(target_os = "windows")]
fn run(title: &str, body: &str) -> std::io::Result<std::process::ExitStatus> {
    use std::os::windows::process::CommandExt;
    // the texts go in through the environment, never into the script itself
    let script = r#"
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
$t = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02)
$x = $t.GetElementsByTagName('text')
$x.Item(0).AppendChild($t.CreateTextNode($env:SHIPOFFICE_TITLE)) | Out-Null
$x.Item(1).AppendChild($t.CreateTextNode($env:SHIPOFFICE_BODY)) | Out-Null
$app = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe'
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($app).Show([Windows.UI.Notifications.ToastNotification]::new($t))
"#;
    const NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("SHIPOFFICE_TITLE", title)
        .env("SHIPOFFICE_BODY", body)
        .creation_flags(NO_WINDOW)
        .status()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn run(_: &str, _: &str) -> std::io::Result<std::process::ExitStatus> {
    Err(std::io::Error::other("no notifier"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_are_one_tidy_line() {
        assert_eq!(clean("a\nb\t c", 80), "a b c");
        assert_eq!(clean(&"x".repeat(100), 10).chars().count(), 10);
    }
}
