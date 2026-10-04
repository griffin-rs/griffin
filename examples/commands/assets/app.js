// The client side of the example: the unmodified Phoenix client, and two client hooks.
// The bundles are the counter example's, served under this example's `/assets/vendor/`.
import { Socket } from "./vendor/phoenix.mjs";
import { LiveSocket } from "./vendor/phoenix_live_view.esm.js";

// A hook is attached by name: `phx-hook="Lookup"` on an element with an `id`.
const hooks = {
  // Pushes an Event to the LiveView and is given the handler's reply.
  Lookup: {
    mounted() {
      this.el.addEventListener("click", () => {
        this.pushEvent("lookup", { id: Number(this.el.dataset.id) }, (reply) => {
          document.getElementById("lookup-result").textContent = `${reply.id}: ${reply.name}`;
        });
      });
    },
  },
  // Listens to an event the server pushes.
  Highlight: {
    mounted() {
      this.handleEvent("highlight", ({ id }) => {
        this.el.textContent = id;
      });
    },
  },
};

// The server only serves a socket that brings back the CSRF token of the page's
// session, which the Dead render left on the LiveView's container.
const container = document.querySelector("[data-phx-main]");
const csrfToken = container.getAttribute("data-csrf-token");

const liveSocket = new LiveSocket("/live", Socket, { hooks, params: { _csrf_token: csrfToken } });
liveSocket.connect();

window.liveSocket = liveSocket;
