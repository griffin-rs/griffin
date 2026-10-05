//! The root layout, which wraps every page, and the pages of errors.

use crate::components::Flash;
use griffin::axum::http::StatusCode;
use griffin::session::Flash as FlashMessages;
use griffin::template::{Rendered, SlotEntries};
use griffin::{component, html};

/// The HTML document around a page.
#[component]
pub fn Document(#[slot] inner_block: SlotEntries<'_>) -> Rendered {
    html! {
        <!DOCTYPE html>
        <html lang="en">
        <head>
            <meta charset="utf-8">
            <meta name="viewport" content="width=device-width, initial-scale=1">
            <title>__App__</title>
            <link rel="stylesheet" href="/assets/app.css">
            <script defer type="module" src="/assets/app.js"></script>
        </head>
        <body>{inner_block}</body>
        </html>
    }
}

/// The layout of the route table: the page, and the flash left by the request before.
pub fn site(page: Rendered, flash: &FlashMessages) -> Rendered {
    html! {
        <Document>
            <Flash message={flash.get("info")} />
            {page}
        </Document>
    }
}

/// The page for an error status, for every status Griffin would otherwise answer with
/// its own plain page. It is given the status and nothing of the failure.
pub fn error_page(status: StatusCode) -> Option<Rendered> {
    let reason = status.canonical_reason().unwrap_or("Error");
    Some(html! {
        <Document>
            <main>
                <h1>{status.as_u16()}</h1>
                <p>{reason}</p>
                <p><a href="/">Back to the home page</a></p>
            </main>
        </Document>
    })
}
