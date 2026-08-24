"use client";

import type { SyntaxHighlighterProps } from "@assistant-ui/react-markdown";
import {
  cloneElement,
  type CSSProperties,
  type FC,
  isValidElement,
  type ReactElement,
} from "react";
import { useShikiHighlighter } from "react-shiki";

import { cn } from "@/lib/utils";

const codeBlockClassName =
  "aui-md-pre border-border/50 bg-muted/30 overflow-x-auto rounded-t-none rounded-b-xl border border-t-0 p-3.5 text-[13px] leading-relaxed";

type HighlightedCodeProps = {
  className?: string;
  style?: CSSProperties;
};

export const SyntaxHighlighter: FC<SyntaxHighlighterProps> = ({
  node,
  components: { Pre, Code },
  language,
  code,
}) => {
  const highlightedCode = useShikiHighlighter(
    code,
    language || "text",
    "vitesse-dark",
    { delay: 150 },
  );

  if (!highlightedCode || !isValidElement(highlightedCode)) {
    return (
      <Pre className={codeBlockClassName}>
        <Code node={node}>{code}</Code>
      </Pre>
    );
  }

  const highlightedElement =
    highlightedCode as ReactElement<HighlightedCodeProps>;

  return cloneElement(highlightedElement, {
    className: cn(codeBlockClassName, highlightedElement.props.className),
  });
};
