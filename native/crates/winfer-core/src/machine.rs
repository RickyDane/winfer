//! What kind of computer this is, for its icon in the device panel: `laptop`, `desktop`, `imac`,
//! `mac-studio`, `mac-mini` or `mac-pro`.

use std::sync::OnceLock;

/// This machine's. Blocking the first time.
pub fn form() -> &'static str {
    static FORM: OnceLock<&str> = OnceLock::new();
    FORM.get_or_init(detect)
}

/// The Mac a product name, model id or computer name speaks of ("Mac Studio von Ricky").
pub fn mac_form(text: &str) -> Option<&'static str> {
    let text: String = text.to_lowercase().chars().filter(char::is_ascii_alphanumeric).collect();
    // `imac` before `macpro`: an "iMac Pro" is an iMac.
    [("macbook", "laptop"), ("imac", "imac"), ("macstudio", "mac-studio"), ("macmini", "mac-mini"), ("macpro", "mac-pro")]
        .into_iter()
        .find(|(model, _)| text.contains(model))
        .map(|(_, form)| form)
}

#[cfg(target_os = "macos")]
fn detect() -> &'static str {
    let run = |cmd: &str, args: &[&str]| std::process::Command::new(cmd).args(args).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    // Apple Silicon's `hw.model` ("Mac14,13") doesn't say which Mac; the product name does.
    // Intel Macs have no product name, but their `hw.model` does ("MacBookPro16,1").
    let product = run("ioreg", &["-rd1", "-c", "IOPlatformDevice", "-k", "product-name"]);
    product.lines().find(|l| l.contains("\"product-name\"")).and_then(mac_form).or_else(|| mac_form(&run("sysctl", &["-n", "hw.model"]))).unwrap_or("desktop")
}

#[cfg(target_os = "windows")]
fn detect() -> &'static str {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", "(Get-CimInstance Win32_SystemEnclosure).ChassisTypes[0]"])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output();
    chassis(&out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn detect() -> &'static str {
    chassis(&std::fs::read_to_string("/sys/class/dmi/id/chassis_type").unwrap_or_default())
}

/// An SMBIOS chassis type: portables, notebooks, tablets and convertibles are laptops.
#[cfg(any(not(target_os = "macos"), test))]
fn chassis(kind: &str) -> &'static str {
    match kind.trim().parse::<u8>() {
        Ok(8..=11 | 14 | 30..=32) => "laptop",
        _ => "desktop",
    }
}

#[cfg(test)]
mod tests {
    use super::{chassis, mac_form};

    #[test]
    fn macs_are_told_apart() {
        assert_eq!(mac_form("Mac Studio von Ricky"), Some("mac-studio"));
        assert_eq!(mac_form("MacBook Pro (16-inch, 2021)"), Some("laptop"));
        assert_eq!(mac_form("Macmini8,1"), Some("mac-mini"));
        assert_eq!(mac_form("iMacPro1,1"), Some("imac"));
        assert_eq!(mac_form("MacPro7,1"), Some("mac-pro"));
        assert_eq!(mac_form("PC-RP-WORKS"), None);
    }

    #[test]
    fn chassis_types() {
        assert_eq!(chassis("10\n"), "laptop");
        assert_eq!(chassis("3"), "desktop");
        assert_eq!(chassis(""), "desktop");
    }
}
