import { cva, type VariantProps } from "class-variance-authority";
import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

const noticeVariants = cva("rounded-control border px-3 py-2 text-sm", {
  variants: {
    tone: {
      neutral: "border-border bg-muted text-foreground",
      info: "border-info/30 bg-info/10 text-info",
      success: "border-success/30 bg-success/10 text-success",
      warning: "border-warning/30 bg-warning/10 text-warning",
      danger: "border-destructive/30 bg-destructive/10 text-destructive",
    },
  },
  defaultVariants: { tone: "neutral" },
});

type NoticeProps = ComponentProps<"div"> & VariantProps<typeof noticeVariants>;

function Notice({ className, tone, ...props }: NoticeProps) {
  return (
    <div
      data-slot="notice"
      role={tone === "danger" ? "alert" : undefined}
      className={cn(noticeVariants({ tone, className }))}
      {...props}
    />
  );
}

export { Notice, noticeVariants };
