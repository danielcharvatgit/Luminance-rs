pub const APP_ID: &str = "io.github.luminance_rs.Luminance";
pub const APP_NAME: &str = "Luminance";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DESCRIPTION: &str = env!("CARGO_PKG_DESCRIPTION");

/// D-Bus object path of the service interface, derived from the app id.
pub fn object_path() -> String {
    format!("/{}/Service", APP_ID.replace('.', "/"))
}
