"use client";

import { memo } from "react";
import {
  unstable_defaultDirectiveFormatter,
  type TextMessagePartComponent,
} from "@assistant-ui/react";
import { FileCode2Icon, HashIcon } from "lucide-react";

const DirectiveTextImpl: TextMessagePartComponent = ({ text }) => {
  const segments = unstable_defaultDirectiveFormatter.parse(text);

  if (segments.length === 1 && segments[0]?.kind === "text") {
    return <>{text}</>;
  }

  return (
    <>
      {segments.map((segment, index) => {
        if (segment.kind === "text") {
          return (
            <span key={index} className="whitespace-pre-wrap">
              {segment.text}
            </span>
          );
        }

        const Icon = segment.type === "file" ? FileCode2Icon : HashIcon;
        return (
          <span
            key={index}
            data-slot="directive-text-chip"
            data-directive-type={segment.type}
            data-directive-id={segment.id}
            aria-label={`${segment.type}: ${segment.label}`}
            className="bg-secondary text-secondary-foreground mx-0.5 inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[13px] leading-none align-baseline"
          >
            <Icon className="size-3" />
            {segment.label}
          </span>
        );
      })}
    </>
  );
};

DirectiveTextImpl.displayName = "DirectiveText";

export const DirectiveText = memo(DirectiveTextImpl);
