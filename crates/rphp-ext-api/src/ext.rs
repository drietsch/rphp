//! Extension metadata: [`ExtInfo`].

/// What `ReflectionExtension` / `phpversion()` / `extension_loaded()` report
/// about an extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtInfo {
    /// The extension name as PHP spells it (`standard`, `SPL`, `PDO`,
    /// `Zend OPcache`); matched case-insensitively by `extension_loaded()`.
    pub name: &'static str,
    /// `phpversion($name)`.
    pub version: &'static str,
    /// Extensions this one requires (`ReflectionExtension::getDependencies()`
    /// entries marked `Required`).
    pub deps: &'static [&'static str],
}
