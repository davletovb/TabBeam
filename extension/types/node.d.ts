declare module "node:assert/strict" {
  interface AssertStrict {
    equal(actual: unknown, expected: unknown): void;
    ok(value: unknown, message?: string): void;
    deepEqual(actual: unknown, expected: unknown): void;
  }

  const assert: AssertStrict;
  export default assert;
}

declare module "node:fs" {
  interface RmOptions {
    recursive?: boolean;
    force?: boolean;
  }

  interface MkdirOptions {
    recursive?: boolean;
  }

  interface CpOptions {
    recursive?: boolean;
  }

  interface FileSystem {
    readFileSync(path: string, encoding: "utf8"): string;
    existsSync(path: string): boolean;
    rmSync(path: string, options?: RmOptions): void;
    mkdirSync(path: string, options?: MkdirOptions): void;
    copyFileSync(source: string, destination: string): void;
    cpSync(source: string, destination: string, options?: CpOptions): void;
  }

  const fs: FileSystem;
  export default fs;
}

declare module "node:path" {
  interface PathApi {
    resolve(...paths: string[]): string;
    dirname(path: string): string;
    join(...paths: string[]): string;
    relative(from: string, to: string): string;
  }

  const path: PathApi;
  export default path;
}

declare module "node:url" {
  export function fileURLToPath(url: string | URL): string;
  export function pathToFileURL(path: string): URL;
}

declare const process: {
  cwd(): string;
};

interface ImportMeta {
  readonly url: string;
}
