declare module "node:assert/strict" {
  interface AssertStrict {
    equal(actual: unknown, expected: unknown, message?: string): void;
    ok(value: unknown, message?: string): void;
    deepEqual(actual: unknown, expected: unknown, message?: string): void;
    throws(block: () => unknown, error?: RegExp | object): void;
  }

  const assert: AssertStrict;
  export default assert;
}

declare module "node:child_process" {
  export function spawn(command: string, args: string[], options: object): any;
}

declare module "node:vm" {
  export function runInNewContext(code: string, context: object): unknown;
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
    readFileSync(path: string | URL, encoding: "utf8"): string;
    mkdtempSync(prefix: string): string;
    existsSync(path: string): boolean;
    rmSync(path: string, options?: RmOptions): void;
    mkdirSync(path: string, options?: MkdirOptions): void;
    copyFileSync(source: string, destination: string): void;
    cpSync(source: string, destination: string, options?: CpOptions): void;
  }

  const fs: FileSystem;
  export default fs;
}

declare module "node:os" {
  interface OsApi {
    tmpdir(): string;
  }

  const os: OsApi;
  export default os;
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
  argv: string[];
  env: Record<string, string | undefined>;
};

interface ImportMeta {
  readonly url: string;
}
