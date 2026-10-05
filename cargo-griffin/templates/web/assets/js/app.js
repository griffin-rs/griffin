// The client side of the app: the unmodified Phoenix client, and nothing else.
import { Socket } from "../vendor/phoenix.mjs";
import { LiveSocket } from "../vendor/phoenix_live_view.esm.js";

// The server only serves a socket that brings back the CSRF token of the page's
// session, which the server left on the LiveView's container. A page with no LiveView
// (the home page, the classic form) has nothing to connect.
const container = document.querySelector("[data-phx-main]");
if (container) {
  const csrfToken = container.getAttribute("data-csrf-token");
  const liveSocket = new LiveSocket("/live", Socket, { params: { _csrf_token: csrfToken } });
  liveSocket.connect();
  window.liveSocket = liveSocket;
}
