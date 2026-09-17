//! ini directives an extension registers: [`IniDef`] / [`IniAccess`].

use crate::bitflags::bitflags;

bitflags! {
    /// Where a directive may be changed (PHP's `INI_USER = 1`, `INI_PERDIR
    /// = 2`, `INI_SYSTEM = 4`, `INI_ALL = 7`), as reported by
    /// `ini_get_all()['access']`.
    pub struct IniAccess: u8 {
        /// Settable from scripts (`ini_set`) — `INI_USER`.
        const USER = 1;
        /// Settable per directory (`.htaccess`, `.user.ini`) — `INI_PERDIR`.
        const PERDIR = 2;
        /// Settable in `php.ini` / the SAPI config only — `INI_SYSTEM`.
        const SYSTEM = 4;
        /// Settable everywhere — `INI_ALL`.
        const ALL = 7;
    }
}

/// An ini directive with its compiled-in default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IniDef {
    /// The directive name (`display_errors`, `session.save_path`).
    pub name: &'static str,
    /// The compiled-in default (`ini_get_all()['global_value']` under
    /// `php -n`), `None` when PHP reports it as unset.
    pub default: Option<&'static str>,
    /// Where the directive may be changed.
    pub access: IniAccess,
}

#[cfg(test)]
mod tests {
    use super::IniAccess;

    #[test]
    fn access_bits_match_php() {
        assert_eq!(
            IniAccess::ALL,
            IniAccess::USER | IniAccess::PERDIR | IniAccess::SYSTEM
        );
        assert_eq!(IniAccess::ALL.bits(), 7);
        assert!(IniAccess::SYSTEM.contains(IniAccess::SYSTEM));
        assert!(!IniAccess::SYSTEM.contains(IniAccess::USER));
    }
}
