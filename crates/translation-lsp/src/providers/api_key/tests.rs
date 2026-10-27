use super::*;

#[test]
fn origins_of_https_and_of_this_computer_name_a_key_file_each() {
    for (url, origin, file) in [
        (
            "https://openrouter.ai/api/v1",
            "https://openrouter.ai",
            "api-key-https-openrouter.ai-443",
        ),
        (
            "https://generativelanguage.googleapis.com/v1beta/openai/",
            "https://generativelanguage.googleapis.com",
            "api-key-https-generativelanguage.googleapis.com-443",
        ),
        (
            "HTTPS://API.OpenAI.com:443/v1",
            "https://api.openai.com",
            "api-key-https-api.openai.com-443",
        ),
        (
            "https://llm.example:8443",
            "https://llm.example:8443",
            "api-key-https-llm.example-8443",
        ),
        (
            "https://bücher.example/v1",
            "https://xn--bcher-kva.example",
            "api-key-https-xn--bcher-kva.example-443",
        ),
        (
            "http://localhost:11434/v1",
            "http://localhost:11434",
            "api-key-http-localhost-11434",
        ),
        (
            "http://127.0.0.1:1234/v1",
            "http://127.0.0.1:1234",
            "api-key-http-127.0.0.1-1234",
        ),
        (
            "http://[::1]:8080",
            "http://[::1]:8080",
            "api-key-http-[__1]-8080",
        ),
        (
            "http://localhost",
            "http://localhost",
            "api-key-http-localhost-80",
        ),
    ] {
        let parsed = Origin::parse(url).unwrap();
        assert_eq!(parsed.as_str(), origin, "{url}");
        let spec = KeySpec::origin("API key", "API key…", parsed);
        assert_eq!(spec.file(), file, "{url}");
    }
    // Origins that differ only in the scheme, or an IPv6 address and a host
    // name written alike, keep their keys apart.
    for (one, other) in [
        ("https://localhost", "http://localhost:443"),
        ("https://[::1]:8080", "https://__1:8080"),
    ] {
        let one = KeySpec::origin("API key", "API key…", Origin::parse(one).unwrap());
        let other = KeySpec::origin("API key", "API key…", Origin::parse(other).unwrap());
        assert_ne!(one.file(), other.file());
    }
    assert_eq!(Origin::openai().as_str(), "https://api.openai.com");
    assert_eq!(
        KeySpec::anthropic("Anthropic API key…").file(),
        "anthropic-api-key"
    );
}

#[test]
fn plain_http_elsewhere_and_other_schemes_are_no_origin() {
    for url in [
        "http://openrouter.ai/api/v1",
        "http://192.168.1.20:11434/v1",
        "http://[2001:db8::1]:8080",
        "http://localhost.example",
        "ftp://localhost",
        "file:///tmp/key",
        "data:text/plain,key",
        "openrouter.ai",
        "",
    ] {
        assert_eq!(Origin::parse(url), None, "{url}");
    }
}

#[test]
fn texts_name_the_key_and_its_file() {
    let spec = KeySpec::origin("OpenAI API key", "OpenAI API key…", Origin::openai());
    let text = |error: Error| error.to_string();
    assert_eq!(
        text(Error::configuration(spec.missing())),
        "OpenAI API key is not saved. Run \"OpenAI API key…\" from the code actions to save it"
    );
    let not_private = text(spec.unusable(CredentialError::Permissions));
    assert!(not_private.starts_with("OpenAI API key file cannot be used. "));
    assert!(not_private.contains(" and api-key-https-api.openai.com-443 in it a regular file"));
    assert!(
        text(spec.unusable(CredentialError::MissingDirectory))
            .starts_with("OpenAI API key file could not be located")
    );
}
