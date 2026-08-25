use tauri::{AppHandle, Emitter};

pub const DEEP_LINK_EVENT: &str = "deep-link://open";
pub const DEEP_LINK_SCHEME: &str = "lazarus://";

pub fn urls_from_argv<I>(argv: I) -> Vec<String>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    argv.into_iter()
        .map(|arg| arg.as_ref().to_string())
        .filter(|arg| arg.starts_with(DEEP_LINK_SCHEME))
        .collect()
}

pub fn emit_from_argv(app: &AppHandle, argv: &[String]) {
    let urls = urls_from_argv(argv.iter().cloned());
    if !urls.is_empty() {
        let _ = app.emit(DEEP_LINK_EVENT, urls);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_only_deep_link_urls() {
        let urls = urls_from_argv([
            "lazarus.exe".to_string(),
            "lazarus://task/abc".to_string(),
            "--flag".to_string(),
            "not-a-link".to_string(),
        ]);
        assert_eq!(urls, vec!["lazarus://task/abc".to_string()]);
    }

    #[test]
    fn empty_argv_yields_no_urls() {
        assert!(urls_from_argv(Vec::<String>::new()).is_empty());
    }
}
