//! First-party bundled extension registration.
//!
//! This crate is the composition root for extensions shipped with AstrCode.
//! `astrcode-extensions` owns the extension runtime, while this crate decides
//! which first-party extensions are linked into a binary.

use astrcode_extensions::runner::ExtensionRunner;

/// Register all enabled first-party bundled extensions in precedence order.
///
/// Earlier registrations keep precedence when multiple extensions expose the
/// same tool name.
pub async fn register_bundled_extensions(runner: &ExtensionRunner) {
    #[cfg(feature = "agent-tools")]
    runner
        .register(astrcode_extension_agent_tools::extension())
        .await;

    #[cfg(feature = "mcp")]
    runner.register(astrcode_extension_mcp::extension()).await;

    #[cfg(feature = "skill")]
    runner.register(astrcode_extension_skill::extension()).await;

    #[cfg(feature = "todo-tool")]
    runner
        .register(astrcode_extension_todo_tool::extension())
        .await;

    #[cfg(feature = "mode")]
    runner.register(astrcode_extension_mode::extension()).await;
}
