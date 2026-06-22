import { AetherEvalError } from "../errors.js";

export interface ImageBuildOptions {
  context: string;
  dockerfile?: string;
  buildArgs?: Record<string, string>;
  cache?: boolean;
  buildkit?: boolean;
  platform?: string;
  target?: string;
  deleteOnExit?: boolean;
}

export class Image {
  readonly name: string;
  readonly tag: string;
  readonly build: ImageBuildOptions | undefined;

  constructor(name: string, tag = "latest", build?: ImageBuildOptions) {
    validateImageReference(`${name}:${tag}`);
    this.name = name;
    this.tag = tag;
    this.build = build ? { ...build } : undefined;
  }

  static parse(image: string): Image {
    const { name, tag } = parseImageReference(image);
    return new Image(name, tag);
  }

  static fromDockerfile(image: string, build: ImageBuildOptions): Image {
    const { name, tag } = parseImageReference(image);
    return new Image(name, tag, build);
  }

  toString(): string {
    return `${this.name}:${this.tag}`;
  }
}

function parseImageReference(image: string): { name: string; tag: string } {
  validateImageReference(image);
  const lastSlash = image.lastIndexOf("/");
  const lastColon = image.lastIndexOf(":");
  if (lastColon !== -1 && (lastSlash === -1 || lastColon > lastSlash)) {
    return {
      name: image.slice(0, lastColon),
      tag: image.slice(lastColon + 1),
    };
  }
  return { name: image, tag: "latest" };
}

function validateImageReference(reference: string): void {
  if (
    reference === "" ||
    reference.startsWith(":") ||
    reference.endsWith(":")
  ) {
    throw new AetherEvalError(
      "invalid_image_reference",
      `invalid container image reference '${reference}'`,
    );
  }
}
