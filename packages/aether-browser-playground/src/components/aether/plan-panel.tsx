import { CheckIcon, CircleDashedIcon, CircleIcon, XIcon } from "lucide-react";
import { useConversation } from "@/aether/store";
import { cn } from "@/lib/utils";

export function PlanPanel() {
  const plan = useConversation()?.plan;
  if (!plan || plan.entries.length === 0) return null;

  return (
    <section className="flex flex-col gap-2 border-b p-4">
      <h2 className="text-sm font-medium">Plan</h2>
      <ol className="flex flex-col gap-1.5 text-sm">
        {plan.entries.map((entry, index) => (
          <li
            key={index}
            className={cn(
              "flex items-start gap-2",
              entry.status === "completed" &&
                "text-muted-foreground line-through",
            )}
          >
            <PlanStatusIcon status={entry.status} />
            {entry.content}
          </li>
        ))}
      </ol>
    </section>
  );
}

function PlanStatusIcon({ status }: { status: string }) {
  const className = "mt-0.5 size-4 shrink-0";
  switch (status) {
    case "completed":
      return <CheckIcon className={className} />;
    case "in_progress":
      return (
        <CircleDashedIcon
          className={cn(className, "animate-spin [animation-duration:3s]")}
        />
      );
    case "cancelled":
      return <XIcon className={className} />;
    default:
      return <CircleIcon className={className} />;
  }
}
