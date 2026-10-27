import { useEffect, useState } from "react";
import { AppVersion } from "./AppVersion.tsx";
import { gatewayFetch, gatewayUrl } from "./gateway.ts";

// The TUI control plane's landing page: its instances, a running one as a link
// that carries this browser's login to the instance's own origin. Started and
// stopped in the TUI, so the list is asked again every two seconds.
type Instance = {
  name: string;
  state: string;
  /** The landing origin's path that opens it; null while it is not running. */
  open: string | null;
};

const REFRESH_MS = 2000;

/** The control plane's instances, or null when this browser is not logged in. */
async function fetchInstances(): Promise<Instance[] | null> {
  const res = await gatewayFetch("/api/instances");
  if (res.status === 401) {
    return null;
  }
  if (!res.ok) {
    throw new Error(`the control plane answered ${res.status}`);
  }
  return (await res.json()) as Instance[];
}

export default function Instances({
  onUnauthorized,
}: {
  onUnauthorized: () => void;
}) {
  const [instances, setInstances] = useState<Instance[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const load = () =>
      fetchInstances().then(
        (list) => {
          if (cancelled) {
            return;
          }
          if (list === null) {
            onUnauthorized();
            return;
          }
          setInstances(list);
          setError(null);
        },
        (cause: unknown) => {
          if (!cancelled) {
            setError(cause instanceof Error ? cause.message : String(cause));
          }
        },
      );
    void load();
    const timer = setInterval(() => void load(), REFRESH_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [onUnauthorized]);

  return (
    <div className="login-screen">
      <div className="instances">
        <h1>zDesk instances</h1>
        <p className="login-hint">Start and stop instances in the zDesk TUI.</p>
        {error && (
          <p className="login-error" role="alert">
            {error}
          </p>
        )}
        {instances?.length === 0 && (
          <p className="login-hint">
            No instances. Press <kbd>n</kbd> in the TUI to create one.
          </p>
        )}
        <ul className="instances-list">
          {instances?.map((instance) => (
            <li key={instance.name}>
              {instance.open ? (
                <a href={gatewayUrl(instance.open)}>{instance.name}</a>
              ) : (
                <span className="instances-name">{instance.name}</span>
              )}
              <span className={`instances-state is-${instance.state}`}>
                {instance.state}
              </span>
            </li>
          ))}
        </ul>
      </div>
      <AppVersion className="login-version" />
    </div>
  );
}
