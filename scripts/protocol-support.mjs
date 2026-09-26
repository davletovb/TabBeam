import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));

export const ROOT = path.resolve(HERE, "..");
export const NATIVE_LITTLE_ENDIAN = os.endianness() === "LE";
export const NATIVE_ENDIAN_NAME = NATIVE_LITTLE_ENDIAN ? "little" : "big";
const UTF8_DECODER = new TextDecoder("utf-8", { fatal: true });

export function decodeUtf8(bytes) {
  return UTF8_DECODER.decode(bytes);
}

export function loadJson(filePath) {
  return JSON.parse(decodeUtf8(fs.readFileSync(filePath)));
}

export function nativeLengthPrefix(length) {
  const prefix = Buffer.alloc(4);
  if (NATIVE_LITTLE_ENDIAN) prefix.writeUInt32LE(length);
  else prefix.writeUInt32BE(length);
  return prefix;
}

export function frameNativeMessage(payload) {
  return Buffer.concat([nativeLengthPrefix(payload.length), Buffer.from(payload)]);
}

export function parseNativeFrames(data) {
  const frames = [];
  let offset = 0;
  while (offset < data.length) {
    if (data.length - offset < 4) {
      throw new Error("host output ended with a partial frame prefix");
    }
    const length = NATIVE_LITTLE_ENDIAN
      ? data.readUInt32LE(offset)
      : data.readUInt32BE(offset);
    offset += 4;
    if (data.length - offset < length) {
      throw new Error("host output ended with a partial frame payload");
    }
    frames.push(JSON.parse(decodeUtf8(data.subarray(offset, offset + length))));
    offset += length;
  }
  return frames;
}

export function sameSet(left, right) {
  return left.size === right.size && [...left].every((value) => right.has(value));
}

function isJsonWhitespace(character) {
  return character === " " || character === "\t" || character === "\n" || character === "\r";
}

function skipWhitespace(source, offset) {
  while (offset < source.length && isJsonWhitespace(source[offset])) offset += 1;
  return offset;
}

function scanString(source, start) {
  if (source[start] !== '"') throw new Error(`expected JSON string at offset ${start}`);
  let offset = start + 1;
  while (offset < source.length) {
    if (source[offset] === "\\") {
      offset += 2;
      continue;
    }
    if (source[offset] === '"') return offset + 1;
    offset += 1;
  }
  throw new Error("unterminated JSON string");
}

function scanValue(source, start) {
  const offset = skipWhitespace(source, start);
  const first = source[offset];

  if (first === '"') return scanString(source, offset);

  if (first === "{") {
    let cursor = skipWhitespace(source, offset + 1);
    if (source[cursor] === "}") return cursor + 1;
    while (cursor < source.length) {
      cursor = scanString(source, cursor);
      cursor = skipWhitespace(source, cursor);
      if (source[cursor] !== ":") throw new Error(`expected ':' at offset ${cursor}`);
      cursor = scanValue(source, cursor + 1);
      cursor = skipWhitespace(source, cursor);
      if (source[cursor] === "}") return cursor + 1;
      if (source[cursor] !== ",") throw new Error(`expected ',' at offset ${cursor}`);
      cursor = skipWhitespace(source, cursor + 1);
    }
    throw new Error("unterminated JSON object");
  }

  if (first === "[") {
    let cursor = skipWhitespace(source, offset + 1);
    if (source[cursor] === "]") return cursor + 1;
    while (cursor < source.length) {
      cursor = scanValue(source, cursor);
      cursor = skipWhitespace(source, cursor);
      if (source[cursor] === "]") return cursor + 1;
      if (source[cursor] !== ",") throw new Error(`expected ',' at offset ${cursor}`);
      cursor = skipWhitespace(source, cursor + 1);
    }
    throw new Error("unterminated JSON array");
  }

  let cursor = offset;
  while (
    cursor < source.length &&
    !isJsonWhitespace(source[cursor]) &&
    ![",", "]", "}"].includes(source[cursor])
  ) {
    cursor += 1;
  }
  if (cursor === offset) throw new Error(`expected JSON value at offset ${offset}`);
  return cursor;
}

function findJsonPath(source, start, segments) {
  const offset = skipWhitespace(source, start);
  if (segments.length === 0) return [offset, scanValue(source, offset)];

  const [segment, ...rest] = segments;
  if (typeof segment === "string") {
    if (source[offset] !== "{") throw new Error(`expected object while looking for ${segment}`);
    let cursor = skipWhitespace(source, offset + 1);
    while (source[cursor] !== "}") {
      const keyStart = cursor;
      const keyEnd = scanString(source, keyStart);
      const key = JSON.parse(source.slice(keyStart, keyEnd));
      cursor = skipWhitespace(source, keyEnd);
      if (source[cursor] !== ":") throw new Error(`expected ':' at offset ${cursor}`);
      const valueStart = skipWhitespace(source, cursor + 1);
      if (key === segment) return findJsonPath(source, valueStart, rest);
      cursor = skipWhitespace(source, scanValue(source, valueStart));
      if (source[cursor] === "}") break;
      if (source[cursor] !== ",") throw new Error(`expected ',' at offset ${cursor}`);
      cursor = skipWhitespace(source, cursor + 1);
    }
    throw new Error(`JSON path member not found: ${segment}`);
  }

  if (!Number.isSafeInteger(segment) || segment < 0) {
    throw new Error(`invalid array index in JSON path: ${segment}`);
  }
  if (source[offset] !== "[") throw new Error(`expected array while looking for index ${segment}`);
  let cursor = skipWhitespace(source, offset + 1);
  let index = 0;
  while (source[cursor] !== "]") {
    const valueStart = cursor;
    if (index === segment) return findJsonPath(source, valueStart, rest);
    cursor = skipWhitespace(source, scanValue(source, valueStart));
    index += 1;
    if (source[cursor] === "]") break;
    if (source[cursor] !== ",") throw new Error(`expected ',' at offset ${cursor}`);
    cursor = skipWhitespace(source, cursor + 1);
  }
  throw new Error(`JSON array index out of range: ${segment}`);
}

export function compactJsonSource(source) {
  let result = "";
  let offset = 0;
  let inString = false;
  while (offset < source.length) {
    const character = source[offset];
    if (inString) {
      result += character;
      if (character === "\\") {
        offset += 1;
        if (offset >= source.length) throw new Error("unterminated JSON escape");
        result += source[offset];
      } else if (character === '"') {
        inString = false;
      }
    } else if (character === '"') {
      inString = true;
      result += character;
    } else if (!isJsonWhitespace(character)) {
      result += character;
    }
    offset += 1;
  }
  if (inString) throw new Error("unterminated JSON string");
  return result;
}

export function jsonValueWireBytes(filePath, pathSegments) {
  const source = decodeUtf8(fs.readFileSync(filePath));
  const [start, end] = findJsonPath(source, 0, pathSegments);
  return Buffer.from(compactJsonSource(source.slice(start, end)), "utf8");
}
