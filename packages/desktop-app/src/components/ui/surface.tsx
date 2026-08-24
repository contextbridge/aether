import { cva, type VariantProps } from "class-variance-authority";
import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

const surfaceVariants = cva("rounded-surface border", {
  variants: {
    variant: {
      default: "bg-surface text-surface-foreground",
      elevated: "bg-surface-elevated text-surface-foreground shadow-surface",
      inset: "bg-surface-sunken text-surface-foreground",
    },
    padding: {
      none: "",
      sm: "p-3",
      md: "p-4",
      lg: "p-6",
    },
  },
  defaultVariants: {
    variant: "default",
    padding: "none",
  },
});

type SurfaceProps = ComponentProps<"div"> &
  VariantProps<typeof surfaceVariants>;

function Surface({ className, variant, padding, ...props }: SurfaceProps) {
  return (
    <div
      data-slot="surface"
      className={cn(surfaceVariants({ variant, padding, className }))}
      {...props}
    />
  );
}

export { Surface, surfaceVariants };
