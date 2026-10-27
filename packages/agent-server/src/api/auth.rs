use super::State;
use axum::{
    extract::{Request, State as ExtractState},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;
use subtle::ConstantTimeEq;

pub(super) fn query_value(query: Option<&str>, key: &str) -> Option<String> {
    url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
}

pub(super) async fn authorize(
    ExtractState(state): ExtractState<Arc<State>>,
    request: Request,
    next: Next,
) -> Response {
    if state.config.auth_token.is_empty() {
        return next.run(request).await;
    }
    let header_token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, value)| value.trim().to_owned());
    let token = header_token.unwrap_or_else(|| {
        query_value(request.uri().query(), "token")
            .filter(|value| !value.is_empty())
            .or_else(|| query_value(request.uri().query(), "access_token"))
            .unwrap_or_default()
            .trim()
            .to_owned()
    });
    let message = if token.is_empty() {
        "missing bearer token"
    } else if !bool::from(token.as_bytes().ct_eq(state.config.auth_token.as_bytes())) {
        "invalid token"
    } else {
        return next.run(request).await;
    };
    (
        StatusCode::UNAUTHORIZED,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        format!("{{\"error\":\"unauthorized\",\"message\":\"{message}\"}}\n"),
    )
        .into_response()
}

pub(super) fn cors_matches(origin: &HeaderValue, patterns: &[String]) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let origin = origin.to_lowercase();
    patterns.iter().any(|pattern| {
        let pattern = pattern.to_lowercase();
        match pattern.split_once('*') {
            Some((prefix, suffix)) => {
                origin.len() >= prefix.len() + suffix.len()
                    && origin.starts_with(prefix)
                    && origin.ends_with(suffix)
            }
            None => origin == pattern,
        }
    })
}

pub(super) fn websocket_origin(headers: &axum::http::HeaderMap, patterns: &[String]) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    if origin.is_empty() {
        return true;
    }
    let Ok(origin) = origin.parse::<axum::http::Uri>() else {
        return false;
    };
    let host = origin
        .authority()
        .map_or("", |authority| authority.as_str());
    if headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .is_some_and(|request| request.eq_ignore_ascii_case(host))
    {
        return true;
    }
    patterns.iter().any(|pattern| {
        let target = if pattern.contains("://") {
            format!("{}://{host}", origin.scheme_str().unwrap_or_default())
        } else {
            host.to_owned()
        };
        goglob::GlobPattern::new(pattern.to_lowercase())
            .is_ok_and(|pattern| pattern.matches(target.to_lowercase()))
    })
}
