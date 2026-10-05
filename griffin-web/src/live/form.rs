//! The params of a form, as the client sends them: those of a form Event, and of a
//! controller's form post.

/// The bytes of a body that is decoded: a larger one is not a form.
pub(crate) const MAX_BYTES: usize = 1 << 20;
// A body keeps `griffin_domain::MAX_PAIRS` pairs and one more, if it has them: that one
// lets `Changeset::cast_form` see that the form has too many, and is invalid.
use griffin_domain::MAX_PAIRS;

/// The params of a form: of a form Event (`phx-change`, `phx-submit`) or of a
/// controller's form post. They are the pairs of the form's
/// URL-encoded body, in order, with their names as the browser wrote them
/// (`user[email]`, `tags[]`) and not yet read into a structure.
///
/// A variant of a LiveView's [Event](super::Event) takes them as a field marked
/// `#[form]`; see [`Event`](macro@super::Event). Nesting and lists are read from the
/// names by `Changeset::cast_form` of `griffin-domain`, which reads only the fields
/// its Changeset declares: nothing a name says can select another.
///
/// The body comes from the browser, so it is bounded, the same way in both crates:
/// at most `MAX_PAIRS` (5000) pairs are kept, and one more, if the body has it
/// ([`truncated`](Self::truncated) says so). The pairs past that are dropped, so that a
/// large form loads and recovers after a reconnect, but `Changeset::cast_form` sees the
/// extra pair and makes the form invalid by itself, so it never applies shortened. A
/// body larger than 1 MiB is not a form, and the Event that carries it cannot be
/// decoded, which ends the LiveView.
///
/// A controller reads a posted form the same way, with `FormParams` as the extractor
/// of its handler (`async fn create(params: FormParams)`): it reads at most 1 MiB of
/// the body, so a larger one is refused with `413 Payload Too Large` before any pair is
/// allocated, then keeps at most 5001 pairs (`MAX_PAIRS` and the one that shows the form is too large). A body that is not
/// `application/x-www-form-urlencoded` is refused with `415 Unsupported Media Type`.
/// Griffin does not wrap axum (it does not duplicate upstream API surface), so this is an extractor of axum and not a
/// replacement of it. Use it and not `axum::Form<Vec<(String, String)>>`, which decodes up to axum's 2 MB
/// default into one owned pair per `&`, about a million for a body of `a&a&a...`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct FormParams(Vec<(String, String)>);

// By hand: the values are what the user typed, passwords included, so only the count
// is shown, as `Session` shows nothing.
impl std::fmt::Debug for FormParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FormParams({} pairs, REDACTED)", self.0.len())
    }
}

impl FormParams {
    /// The pairs of a URL-encoded body, or `None` if it is larger than a form is.
    pub(super) fn parse(body: &str) -> Option<FormParams> {
        (body.len() <= MAX_BYTES).then(|| FormParams::parse_bytes(body.as_bytes()))
    }

    /// The pairs of a body already known to be within `MAX_BYTES`.
    fn parse_bytes(body: &[u8]) -> FormParams {
        let pairs = form_urlencoded::parse(body)
            .take(MAX_PAIRS + 1)
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();
        FormParams(pairs)
    }

    /// Whether the body had more pairs than a form may have (`MAX_PAIRS`), so that
    /// the pairs past the limit were dropped. A read-only signal: `cast_form` finds it
    /// out itself.
    pub fn truncated(&self) -> bool {
        self.0.len() > MAX_PAIRS
    }

    /// Each name and value, in the order the browser sent them: the pairs kept, at most
    /// `MAX_PAIRS + 1`. The last of that many is what tells `Changeset::cast_form` that
    /// the form is too large, so pass them all on.
    pub fn pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }
}

/// The body of a posted form, bounded before it is read into pairs.
impl<S: Send + Sync> axum::extract::FromRequest<S> for FormParams {
    type Rejection = axum::http::StatusCode;

    async fn from_request(request: axum::extract::Request, _: &S) -> Result<Self, Self::Rejection> {
        use axum::http::{StatusCode, header};
        let urlencoded = request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|media| {
                media
                    .trim()
                    .eq_ignore_ascii_case("application/x-www-form-urlencoded")
            });
        if !urlencoded {
            return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
        }
        // The read stops at the limit, whatever the body claims its length is.
        let bytes = axum::body::to_bytes(request.into_body(), MAX_BYTES)
            .await
            .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
        Ok(FormParams::parse_bytes(&bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_shows_no_name_or_value() {
        let params = FormParams::parse("user%5Bpassword%5D=hunter2").unwrap();
        let shown = format!("{params:?}");
        assert!(
            !shown.contains("hunter2") && !shown.contains("password"),
            "{shown}"
        );
    }

    #[test]
    fn pairs_past_the_limit_are_dropped() {
        let body = (0..=MAX_PAIRS)
            .map(|n| format!("a{n}=x"))
            .collect::<Vec<_>>()
            .join("&");
        let params = FormParams::parse(&body).unwrap();
        assert!(params.truncated());
        assert_eq!(params.pairs().count(), MAX_PAIRS + 1);
        assert!(!FormParams::parse("a=x").unwrap().truncated());
    }

    #[test]
    fn a_form_that_is_not_a_string_names_its_field() {
        use crate::live::{EventError, Payload};
        let value = serde_json::json!({"a": 1});
        let error = Payload::from(&value).form("signup").unwrap_err();
        assert_eq!(error, EventError::Form("signup"));
        assert!(error.to_string().contains("`signup`"), "{error}");
    }
}
