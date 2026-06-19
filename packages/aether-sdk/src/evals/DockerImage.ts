import { AetherSdkError } from "../errors.js";

export interface DockerImageBuildOptions {
  context: string;
  dockerfile?: string;
  buildArgs?: Record<string, string>;
  cache?: boolean;
  buildkit?: boolean;
  platform?: string;
  target?: string;
  deleteOnExit?: boolean;
}

export class DockerImage {
  readonly name: string;
  readonly tag: string;
  readonly build: DockerImageBuildOptions | undefined;

  constructor(name: string, tag = "latest", build?: DockerImageBuildOptions) {
    validateImageReference(`${name}:${tag}`);
    this.name = name;
    this.tag = tag;
    this.build = build ? { ...build } : undefined;
  }

  static parse(image: string): DockerImage {
    const { name, tag } = parseImageReference(image);
    return new DockerImage(name, tag);
  }

  static fromDockerfile(
    image: string,
    build: DockerImageBuildOptions,
  ): DockerImage {
    const { name, tag } = parseImageReference(image);
    return new DockerImage(name, tag, build);
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
    throw new AetherSdkError(
      "invalid_options",
      `invalid Docker image reference '${reference}'`,
    );
  }
}
