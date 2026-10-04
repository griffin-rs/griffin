// The client side of the example: the unmodified Phoenix client, and nothing else.
// The bundles are the counter example's, served under this example's `/assets/vendor/`.
import { Socket } from "./vendor/phoenix.mjs";
import { LiveSocket } from "./vendor/phoenix_live_view.esm.js";

// The server only serves a socket that brings back the CSRF token of the page's
// session, which the Dead render left on the LiveView's container. The controller
// form's page has no LiveView, and so nothing to connect.
const container = document.querySelector("[data-phx-main]");
if (container) {
  const csrfToken = container.getAttribute("data-csrf-token");
  const liveSocket = new LiveSocket("/live", Socket, { params: { _csrf_token: csrfToken } });
  liveSocket.connect();
  window.liveSocket = liveSocket;
}
