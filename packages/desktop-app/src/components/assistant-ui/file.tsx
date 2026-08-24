"use client";

import { memo, type FC } from "react";
import {
  unstable_defaultDirectiveFormatter,
  useAui,
  useAuiState,
} from "@assistant-ui/react";
import { cva, type VariantProps } from "class-variance-authority";
import {
  FileIcon,
  FileTextIcon,
  ImageIcon,
  MusicIcon,
  VideoIcon,
  BracesIcon,
  DownloadIcon,
  XIcon,
} from "lucide-react";
import type { FileMessagePartComponent } from "@assistant-ui/react";
import { cn } from "@/lib/utils";

const fileVariants = cva(
  "aui-file-root inline-flex items-center gap-3 rounded-lg transition-colors",
  {
    variants: {
      variant: {
        outline: "border-border hover:bg-muted/50 border",
        ghost: "hover:bg-muted/50",
        muted: "bg-muted/50 hover:bg-muted/70",
      },
      size: {
        sm: "px-2.5 py-1.5 text-xs",
        default: "px-3 py-2 text-sm",
        lg: "px-4 py-3 text-base",
      },
    },
    defaultVariants: {
      variant: "outline",
      size: "default",
    },
  },
);

function getMimeTypeIcon(mimeType: string): FC<{ className?: string }> {
  const type = mimeType.toLowerCase();
  if (type.startsWith("image/")) {
    return ImageIcon;
  }
  if (type === "application/pdf") {
    return FileTextIcon;
  }
  if (type === "application/json") {
    return BracesIcon;
  }
  if (type.startsWith("text/")) {
    return FileTextIcon;
  }
  if (type.startsWith("audio/")) {
    return MusicIcon;
  }
  if (type.startsWith("video/")) {
    return VideoIcon;
  }
  return FileIcon;
}

export type FileDataKind = "data-uri" | "url" | "base64" | "id";

function getFileDataKind(
  data: string,
  sourceType?: "url" | "id",
): FileDataKind {
  if (sourceType === "url" && /^data:/i.test(data)) return "data-uri";
  if (sourceType) return sourceType;
  if (/^data:/i.test(data)) return "data-uri";
  if (/^https?:\/\//i.test(data)) return "url";
  return "base64";
}

function getBase64Size(base64: string): number {
  const commaIndex = base64.indexOf(",");
  const base64Data = commaIndex >= 0 ? base64.slice(commaIndex + 1) : base64;
  const padding = (base64Data.match(/=/g) || []).length;
  return Math.floor((base64Data.length * 3) / 4) - padding;
}

function formatFileSize(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(1)} KB`;
  }
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export type FileRootProps = React.ComponentProps<"div"> &
  VariantProps<typeof fileVariants>;

type MentionedFile = {
  id: string;
  label: string;
  directive: string;
};

const mimeTypeForFilename = (filename: string): string => {
  const extension = filename.split(".").pop()?.toLowerCase();
  const mimeTypes: Record<string, string> = {
    css: "text/css",
    html: "text/html",
    js: "text/javascript",
    json: "application/json",
    md: "text/markdown",
    py: "text/x-python",
    rs: "text/x-rust",
    ts: "text/typescript",
    tsx: "text/typescript",
    txt: "text/plain",
    yaml: "application/yaml",
    yml: "application/yaml",
  };
  return mimeTypes[extension ?? ""] ?? "application/octet-stream";
};

const mentionedFilesInText = (text: string): MentionedFile[] =>
  unstable_defaultDirectiveFormatter.parse(text).flatMap((segment) =>
    segment.kind === "mention" && segment.type === "file"
      ? [
          {
            id: segment.id,
            label: segment.label,
            directive: `:file[${segment.label}]{name=${segment.id}}`,
          },
        ]
      : [],
  );

const removeMentionedFile = (text: string, directive: string): string => {
  const index = text.indexOf(directive);
  if (index === -1) return text;

  const before = text.slice(0, index);
  const after = text.slice(index + directive.length);
  if (before.endsWith(" ") && after.startsWith(" ")) {
    return before + after.slice(1);
  }
  if (before.length === 0 && after.startsWith(" ")) return after.slice(1);
  if (after.length === 0 && before.endsWith(" ")) return before.slice(0, -1);
  return before + after;
};

function FileRoot({
  className,
  variant,
  size,
  children,
  ...props
}: FileRootProps) {
  return (
    <div
      data-slot="file-root"
      data-variant={variant}
      data-size={size}
      className={cn(fileVariants({ variant, size, className }))}
      {...props}
    >
      {children}
    </div>
  );
}

type FileIconDisplayProps = React.ComponentProps<"span"> & {
  mimeType?: string;
};

function FileIconDisplay({
  mimeType,
  className,
  children,
  ...props
}: FileIconDisplayProps) {
  const IconComponent = mimeType ? getMimeTypeIcon(mimeType) : FileIcon;

  return (
    <span
      data-slot="file-icon"
      className={cn("text-muted-foreground shrink-0", className)}
      {...props}
    >
      {children ?? <IconComponent className="size-5" />}
    </span>
  );
}

function FileName({
  className,
  children,
  ...props
}: React.ComponentProps<"span">) {
  return (
    <span
      data-slot="file-name"
      className={cn("min-w-0 flex-1 truncate font-medium", className)}
      {...props}
    >
      {children || "Unnamed file"}
    </span>
  );
}

type FileSizeProps = React.ComponentProps<"span"> & {
  bytes: number;
};

function FileSize({ bytes, className, ...props }: FileSizeProps) {
  return (
    <span
      data-slot="file-size"
      className={cn("text-muted-foreground shrink-0", className)}
      {...props}
    >
      {formatFileSize(bytes)}
    </span>
  );
}

type FileDownloadProps = Omit<React.ComponentProps<"a">, "href"> & {
  data: string;
  mimeType: string;
  filename?: string;
  sourceType?: "url" | "id";
};

function FileDownload({
  data,
  mimeType,
  filename,
  sourceType,
  className,
  children,
  ...props
}: FileDownloadProps) {
  if (typeof data !== "string") return null;
  const kind = getFileDataKind(data, sourceType);
  if (kind === "id") return null;
  if (kind === "url" && !/^(https?:\/\/|blob:)/i.test(data)) return null;
  const href = kind === "base64" ? `data:${mimeType};base64,${data}` : data;

  return (
    <a
      data-slot="file-download"
      href={href}
      download={filename || "download"}
      {...(kind === "url" && { target: "_blank", rel: "noopener noreferrer" })}
      className={cn(
        "text-muted-foreground hover:bg-accent hover:text-accent-foreground shrink-0 rounded-md p-1 transition-colors",
        className,
      )}
      {...props}
    >
      {children || <DownloadIcon className="size-4" />}
    </a>
  );
}

export const ComposerMentionedFiles: FC = () => {
  const aui = useAui();
  const text = useAuiState((state) => state.composer.text);
  const files = mentionedFilesInText(text);

  if (files.length === 0) return null;

  return (
    <div className="aui-composer-mentioned-files flex w-full flex-row flex-wrap items-center gap-2">
      {files.map((file, index) => (
        <File.Root
          key={`${file.id}-${index}`}
          variant="muted"
          size="sm"
          className="max-w-full"
          aria-label={`Included file: ${file.label}`}
        >
          <File.Icon mimeType={mimeTypeForFilename(file.id)} />
          <File.Name>{file.label}</File.Name>
          <button
            type="button"
            aria-label={`Remove included file: ${file.label}`}
            className="text-muted-foreground hover:bg-accent hover:text-accent-foreground -me-1 shrink-0 rounded-md p-1 transition-colors"
            onClick={() =>
              aui.composer.setText(removeMentionedFile(text, file.directive))
            }
          >
            <XIcon className="size-3.5" />
          </button>
        </File.Root>
      ))}
    </div>
  );
};

const FileImpl: FileMessagePartComponent = ({
  filename,
  data,
  mimeType,
  sourceType,
}) => {
  const kind = getFileDataKind(data, sourceType);
  const showSize =
    typeof data === "string" && (kind === "base64" || kind === "data-uri");

  return (
    <FileRoot>
      <FileIconDisplay mimeType={mimeType} />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <FileName>{filename}</FileName>
        {showSize && (
          <FileSize bytes={getBase64Size(data)} className="text-xs" />
        )}
      </div>
      <FileDownload
        data={data}
        mimeType={mimeType}
        {...(filename !== undefined && { filename })}
        {...(sourceType !== undefined && { sourceType })}
      />
    </FileRoot>
  );
};

const File = memo(FileImpl) as unknown as FileMessagePartComponent & {
  Root: typeof FileRoot;
  Icon: typeof FileIconDisplay;
  Name: typeof FileName;
  Size: typeof FileSize;
  Download: typeof FileDownload;
};

File.displayName = "File";
File.Root = FileRoot;
File.Icon = FileIconDisplay;
File.Name = FileName;
File.Size = FileSize;
File.Download = FileDownload;

export {
  File,
  FileRoot,
  FileIconDisplay,
  FileName,
  FileSize,
  FileDownload,
  fileVariants,
  getMimeTypeIcon,
  getFileDataKind,
  getBase64Size,
  formatFileSize,
};
