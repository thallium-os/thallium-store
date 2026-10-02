//! Host package-manager detection.
//!
//! Thallium Store's `system` source is whatever the host distro manages its
//! packages with. Debian-family hosts keep the original apt path untouched;
//! a host with pacman and no apt-get is treated as Arch, which also enables
//! the AUR source. Detection is a PATH scan, never a subprocess.

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageManager {
    Apt,
    Pacman,
}

impl PackageManager {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apt => "apt",
            Self::Pacman => "pacman",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AurHelper {
    Paru,
    Yay,
}

impl AurHelper {
    pub fn binary(self) -> &'static str {
        match self {
            Self::Paru => "paru",
            Self::Yay => "yay",
        }
    }
}

/// Shown wherever the AUR is wanted but no helper is installed.
pub const NO_AUR_HELPER: &str = "Install paru or yay to use the AUR";

/// pacman present and apt-get absent means pacman; anything else keeps apt,
/// so a Debian host (or one with both) behaves exactly as before.
pub fn detect_package_manager(has: impl Fn(&str) -> bool) -> PackageManager {
    if has("pacman") && !has("apt-get") {
        PackageManager::Pacman
    } else {
        PackageManager::Apt
    }
}

/// paru first, then yay.
pub fn detect_aur_helper(has: impl Fn(&str) -> bool) -> Option<AurHelper> {
    [AurHelper::Paru, AurHelper::Yay]
        .into_iter()
        .find(|helper| has(helper.binary()))
}

/// The host package manager, detected once per process.
pub fn package_manager() -> PackageManager {
    static DETECTED: OnceLock<PackageManager> = OnceLock::new();
    *DETECTED.get_or_init(|| detect_package_manager(on_path))
}

pub fn is_pacman_host() -> bool {
    package_manager() == PackageManager::Pacman
}

/// The installed AUR helper. Not cached: someone may install paru while the
/// daemon is running, and the scan is cheap.
pub fn aur_helper() -> Option<AurHelper> {
    detect_aur_helper(on_path)
}

/// Whether `bin` resolves to a file on PATH.
pub fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(bins: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |bin| bins.contains(&bin)
    }

    #[test]
    fn pacman_without_apt_is_pacman() {
        assert_eq!(
            detect_package_manager(only(&["pacman"])),
            PackageManager::Pacman
        );
    }

    #[test]
    fn apt_wins_whenever_present() {
        assert_eq!(
            detect_package_manager(only(&["apt-get"])),
            PackageManager::Apt
        );
        assert_eq!(
            detect_package_manager(only(&["apt-get", "pacman"])),
            PackageManager::Apt
        );
        assert_eq!(detect_package_manager(only(&[])), PackageManager::Apt);
    }

    #[test]
    fn aur_helper_prefers_paru_then_yay() {
        assert_eq!(
            detect_aur_helper(only(&["paru", "yay"])),
            Some(AurHelper::Paru)
        );
        assert_eq!(detect_aur_helper(only(&["yay"])), Some(AurHelper::Yay));
        assert_eq!(detect_aur_helper(only(&["pacman"])), None);
    }
}
