import { useCallback, useEffect, useState } from "react";
import { standAlone } from "./displayTab.ts";
import { gatewayFetch, gatewayUrl } from "./gateway.ts";
import { gatewayConfig } from "./gatewayConfig.ts";
import Instances from "./Instances.tsx";
import Login from "./Login.tsx";
import PinLogin from "./PinLogin.tsx";
import RemoteDesktop from "./RemoteDesktop.tsx";
import { MUTED_KEY, SESSION_KEY } from "./useRemoteDesktop.ts";

// The display a page at `/display/2` shows in a tab of its own, beside the session
// another tab of this browser holds; null for every other path, which is the page
// that holds the session. Two is the only one: a session lays out two displays at
// most, and the first is the one on the session's own page.
function tabDisplayOf(path: string): number | null {
  return /^\/display\/2\/?$/.test(path) ? 2 : null;
}

const TAB_DISPLAY = tabDisplayOf(globalThis.location?.pathname ?? "/");

// A display's tab opened from the session's page starts with a copy of that
// page's storage (displayTab.ts). What is the session's goes before anything
// here reads it: the claim, which this page never makes, and the choice of
// sound, which is that page's.
if (TAB_DISPLAY !== null) {
  standAlone([SESSION_KEY, MUTED_KEY]);
}

// Gate the desktop behind the login. The desktop is only mounted once
// authenticated — mounting it claims the session slot, which must not happen
// before the login succeeds.
//
// Which login is the gateway's to say, on /api/auth/status: a served gateway's
// username and password, or, behind the TUI's control plane, the PIN its access
// panel shows, which only the control plane's own origin asks for. That origin
// is its landing page, the list of its instances, rather than a desktop.
type Gate =
  | { kind: "checking" }
  | { kind: "password" }
  | { kind: "pin"; problem: string | null }
  | { kind: "instances" }
  | { kind: "desktop" };

type AuthStatus = {
  authenticated: boolean;
  /** "pin" on the TUI control plane's own origin; absent on a gateway. */
  login?: "pin";
  /** Why no PIN can log anyone in right now, if so. */
  problem?: string | null;
};

function gateOf(status: AuthStatus): Gate {
  if (status.login === "pin") {
    return status.authenticated
      ? { kind: "instances" }
      : { kind: "pin", problem: status.problem ?? null };
  }
  return status.authenticated ? { kind: "desktop" } : { kind: "password" };
}

export default function App() {
  const [gate, setGate] = useState<Gate>({ kind: "checking" });
  // Deployment branding (login screen, interstitials, tab title). Defaults to
  // "zDesk" and stays there until GET /api/config answers — a public route,
  // so it resolves before login.
  const [branding, setBranding] = useState("zDesk");

  useEffect(() => {
    let cancelled = false;
    gatewayConfig().then(({ branding, logo }) => {
      if (cancelled) {
        return;
      }
      if (branding) {
        setBranding(branding);
        document.title = branding;
      }
      // The tab's icon. There is no <link rel="icon"> in index.html to fight
      // with — a gateway without a logo keeps no icon at all — so this only ever
      // adds one, and never needs removing: the config is fetched once per page.
      if (logo) {
        const link = document.createElement("link");
        link.rel = "icon";
        link.href = gatewayUrl("/api/logo");
        document.head.appendChild(link);
      }
    });
    return () => {
      cancelled = true;
    };
  }, []);

  // Asked on load, and again whenever the answer may have changed: after a
  // logout, and on a 401 mid-session, when a restart wiped the in-memory
  // logins — which, behind the TUI, is the control plane asking for the PIN
  // where the instance's gateway had been answering.
  const check = useCallback(() => {
    setGate({ kind: "checking" });
    gatewayFetch("/api/auth/status")
      .then((res) => res.json() as Promise<AuthStatus>)
      .then((status) => setGate(gateOf(status)))
      .catch(() => setGate({ kind: "password" }));
  }, []);

  useEffect(check, [check]);

  // Log out: end this browser's login. The slot token goes too, so
  // the next login claims fresh instead of silently reattaching.
  const logout = useCallback(() => {
    sessionStorage.removeItem(SESSION_KEY);
    void gatewayFetch("/api/auth/logout", { method: "POST" }).finally(check);
  }, [check]);

  switch (gate.kind) {
    case "checking":
    case "password":
      return (
        <Login
          checking={gate.kind === "checking"}
          branding={branding}
          onLogin={() => setGate({ kind: "desktop" })}
        />
      );
    case "pin":
      return <PinLogin branding={branding} problem={gate.problem} />;
    case "instances":
      return <Instances onUnauthorized={check} />;
  }
  return (
    <RemoteDesktop
      branding={branding}
      tabDisplay={TAB_DISPLAY}
      onLogout={logout}
      onUnauthorized={check}
    />
  );
}
