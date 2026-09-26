//! The browser a raw user agent names.

/// Product tokens in the order they identify a browser. Every Chromium browser also
/// claims `Chrome/` and `Safari/`, so its own token comes first; Safari alone carries
/// `Version/`.
const PRODUCTS: [&[&str]; 5] = [
    &["Edg/"],
    &["OPR/"],
    &["Firefox/"],
    &["Chrome/"],
    &["Version/", "Safari/"],
];

/// `Firefox/130.0 · Macintosh`: the browser's product token and the first field of the
/// system comment. `None` for an agent none of [`PRODUCTS`] names.
pub fn product(agent: &str) -> Option<String> {
    let named = PRODUCTS.iter().find_map(|names| {
        let tokens: Option<Vec<&str>> = names
            .iter()
            .map(|name| {
                agent
                    .split_whitespace()
                    .find(|token| token.starts_with(name))
            })
            .collect();
        tokens.map(|tokens| tokens.join(" "))
    })?;
    match system(agent) {
        Some(system) => Some(format!("{named} · {system}")),
        None => Some(named),
    }
}

/// `Windows NT 10.0` from `Mozilla/5.0 (Windows NT 10.0; Win64; x64) …`.
fn system(agent: &str) -> Option<&str> {
    let (_, comment) = agent.split_once('(')?;
    let system = comment.split([';', ')']).next()?.trim();
    (!system.is_empty()).then_some(system)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firefox_is_named_by_its_own_token() {
        assert_eq!(
            product(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:130.0) Gecko/20100101 \
                 Firefox/130.0"
            )
            .as_deref(),
            Some("Firefox/130.0 · Macintosh")
        );
    }

    #[test]
    fn safari_is_named_by_its_version_and_safari_tokens() {
        assert_eq!(
            product(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
                 (KHTML, like Gecko) Version/17.5 Safari/605.1.15"
            )
            .as_deref(),
            Some("Version/17.5 Safari/605.1.15 · Macintosh")
        );
    }

    #[test]
    fn edge_is_named_before_the_chrome_it_also_claims() {
        assert_eq!(
            product(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like \
                 Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.2849.68"
            )
            .as_deref(),
            Some("Edg/130.0.2849.68 · Windows NT 10.0")
        );
    }

    #[test]
    fn chrome_is_not_mistaken_for_the_safari_it_claims() {
        assert_eq!(
            product(
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/130.0.0.0 Safari/537.36"
            )
            .as_deref(),
            Some("Chrome/130.0.0.0 · X11")
        );
    }

    #[test]
    fn an_agent_without_a_known_product_names_nothing() {
        assert_eq!(product("curl/8.9.1"), None);
    }

    #[test]
    fn an_agent_without_a_system_comment_is_the_product_alone() {
        assert_eq!(product("Firefox/130.0").as_deref(), Some("Firefox/130.0"));
    }
}
