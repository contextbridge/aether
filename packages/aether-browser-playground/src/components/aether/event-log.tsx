import { useState } from "react";
import { type LoggedEvent, useAether } from "@/aether/store";

/** Every event the client delivered, newest first, as the JSON the page received. */
export function EventLog() {
  const events = useAether((state) => state.events);
  const [showSnapshots, setShowSnapshots] = useState(false);
  const shown = events
    .filter(
      ({ event }) => showSnapshots || event.type !== "conversation_changed",
    )
    .toReversed();

  return (
    <section className="flex min-h-0 flex-1 flex-col">
      <div className="flex items-center justify-between border-b px-4 py-2">
        <h2 className="text-sm font-medium">Events ({events.length})</h2>
        <label className="text-muted-foreground flex items-center gap-1.5 text-xs">
          <input
            type="checkbox"
            checked={showSnapshots}
            onChange={(event) => setShowSnapshots(event.target.checked)}
          />
          conversation_changed
        </label>
      </div>
      <ol className="min-h-0 flex-1 overflow-y-auto">
        {shown.map((logged) => (
          <EventRow key={logged.seq} logged={logged} />
        ))}
      </ol>
    </section>
  );
}

function EventRow({ logged: { seq, at, event } }: { logged: LoggedEvent }) {
  return (
    <li className="border-b px-4 py-1.5">
      <details>
        <summary className="cursor-pointer font-mono text-xs">
          <span className="text-muted-foreground">
            #{seq} {at.toLocaleTimeString()}
          </span>{" "}
          {event.type}
          {event.type === "session_update" &&
            ` · ${event.notification.update.sessionUpdate}`}
        </summary>
        <pre className="bg-muted/50 mt-1 max-h-96 overflow-auto rounded-md p-2 text-xs">
          {JSON.stringify(event, null, 2)}
        </pre>
      </details>
    </li>
  );
}
