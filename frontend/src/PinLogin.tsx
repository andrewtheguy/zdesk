import { type FormEvent, useState } from "react";
import { AppVersion } from "./AppVersion.tsx";
import { gatewayFetch } from "./gateway.ts";

// The TUI control plane's login gate: the PIN its access panel shows, entered
// on whichever of its origins the browser is at. POST /api/auth/pin sets the
// login cookie for the landing origin and every instance origin, and the page
// then reloads where it is: on the landing page that is the list of instances,
// and on an instance origin it is that instance's gateway.
export default function PinLogin({
  branding,
  problem,
}: {
  /** Display name shown as the form heading. */
  branding: string;
  /** Why no PIN can log anyone in right now (expired, withdrawn), if so. */
  problem: string | null;
}) {
  const [pin, setPin] = useState("");
  const [error, setError] = useState<string | null>(problem);
  const [submitting, setSubmitting] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      const res = await gatewayFetch("/api/auth/pin", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ pin }),
      });
      if (res.ok) {
        globalThis.location.reload();
        return;
      }
      const body = (await res.json().catch(() => ({}))) as { error?: string };
      setError(body.error ?? `Login failed (${res.status})`);
      setPin("");
    } catch (cause) {
      setError(
        cause instanceof Error ? cause.message : "Could not reach the server",
      );
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="login-screen">
      <form className="login-form" onSubmit={(e) => void submit(e)}>
        <h1>{branding}</h1>
        <p className="login-hint pin-hint">
          Enter the PIN shown in the access panel of the zDesk TUI.
        </p>
        {error && (
          <p className="login-error" role="alert">
            {error}
          </p>
        )}
        <label htmlFor="login-pin">PIN</label>
        <input
          id="login-pin"
          className="pin-input"
          type="text"
          value={pin}
          // The TUI shows and copies the PIN as 123-456; the field keeps the
          // digits and sends those. No maxLength: a browser cuts a paste to it
          // before this runs, which would lose the last digit of 123-456.
          onChange={(e) =>
            setPin(e.target.value.replace(/\D/g, "").slice(0, 6))
          }
          inputMode="numeric"
          autoComplete="one-time-code"
          pattern="[0-9]{6}"
          required
          // biome-ignore lint/a11y/noAutofocus: the PIN is the only thing this screen asks for
          autoFocus
          disabled={submitting}
        />
        <button type="submit" disabled={submitting || pin.length !== 6}>
          {submitting ? "Logging in…" : "Log in"}
        </button>
      </form>
      <AppVersion className="login-version" />
    </div>
  );
}
