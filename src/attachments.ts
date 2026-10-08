import { formatCsvAttachment, parseCsvAttachment } from "./csv-attachment";

export { parseCsvAttachment } from "./csv-attachment";

export type AttachmentKind = "pdf" | "text" | "csv" | "image" | "audio" | "video";
type AttachmentRejection = "archive" | "binary";

export type AttachmentResolution =
    { kind: AttachmentKind; mime: string } | { kind: "unsupported"; reason: AttachmentRejection; mime: string };

type AttachmentMeta = {
    lines?: number;
    rows?: number;
    columns?: number;
    pages?: number;
    extractedPages?: number;
    language?: string;
    preview?: string;
    thumbnail?: string;
    truncated?: boolean;
};

export type StagedAttachment = {
    id: string;
    filename: string;
    mime: string;
    size: number;
    status: "processing" | "ready";
    dataUrl?: string;
    text?: string;
    meta: AttachmentMeta;
};

export type AttachmentFailure = "archive" | "binary" | "too-large" | "invalid-utf8" | "read-failed";
export type AttachmentPreparation =
    | { ok: true; attachment: StagedAttachment }
    | { ok: false; reason: AttachmentFailure; kind?: AttachmentKind; limit?: number };

const mib = 1024 * 1024;
export const attachmentSizeLimits: Record<AttachmentKind, number> = {
    text: 2 * mib,
    csv: 5 * mib,
    image: 10 * mib,
    pdf: 20 * mib,
    audio: 20 * mib,
    video: 40 * mib,
};

const maxTextAttachmentChars = 120_000;
export const maxPdfExtractionPages = 12;
export const maxPdfExtractionChars = 100_000;

const archiveExtensions = new Set([
    "7z",
    "apk",
    "bz2",
    "cab",
    "dmg",
    "gz",
    "iso",
    "jar",
    "rar",
    "tar",
    "tgz",
    "war",
    "xz",
    "zip",
]);
const binaryExtensions = new Set([
    "bin",
    "class",
    "com",
    "db",
    "dll",
    "dylib",
    "exe",
    "img",
    "msi",
    "o",
    "obj",
    "pdb",
    "so",
    "sqlite",
    "sqlite3",
    "wasm",
    "xls",
    "xlsx",
]);
const imageExtensions = new Set(["avif", "bmp", "gif", "heic", "heif", "jpeg", "jpg", "png", "svg", "webp"]);
const audioExtensions = new Set(["aac", "flac", "m4a", "mp3", "oga", "ogg", "opus", "wav", "weba"]);
const videoExtensions = new Set(["avi", "m4v", "mkv", "mov", "mp4", "mpeg", "mpg", "ogv", "webm"]);
const csvExtensions = new Set(["csv", "tsv"]);
const textExtensions = new Set([
    "c",
    "cc",
    "cfg",
    "conf",
    "cpp",
    "cs",
    "css",
    "dart",
    "dockerignore",
    "editorconfig",
    "env",
    "gitattributes",
    "gitignore",
    "go",
    "graphql",
    "h",
    "hpp",
    "htm",
    "html",
    "ini",
    "java",
    "js",
    "json",
    "jsonl",
    "jsx",
    "kt",
    "less",
    "log",
    "lua",
    "md",
    "mdx",
    "mjs",
    "npmrc",
    "php",
    "pl",
    "properties",
    "rc",
    "ps1",
    "py",
    "rb",
    "rs",
    "sass",
    "scss",
    "sh",
    "sql",
    "svelte",
    "swift",
    "tex",
    "toml",
    "ts",
    "tsx",
    "txt",
    "vue",
    "xml",
    "yaml",
    "yml",
    "zsh",
]);
const textBasenames = new Set(["dockerfile", "gemfile", "justfile", "license", "makefile", "procfile", "readme"]);

const textualApplicationMimes = new Set([
    "application/graphql",
    "application/javascript",
    "application/json",
    "application/ld+json",
    "application/sql",
    "application/toml",
    "application/x-httpd-php",
    "application/x-javascript",
    "application/x-ndjson",
    "application/x-sh",
    "application/xhtml+xml",
    "application/xml",
    "application/yaml",
]);

function extension(filename?: string) {
    return (
        filename
            ?.replaceAll("\\", "/")
            .split("/")
            .at(-1)
            ?.match(/\.([^.]+)$/)?.[1]
            ?.toLowerCase() ?? ""
    );
}

function normalizedMime(mime?: string) {
    return (mime ?? "").split(";", 1)[0].trim().toLowerCase();
}

function starts(bytes: Uint8Array | undefined, values: number[]) {
    return !!bytes && values.every((value, index) => bytes[index] === value);
}

export function resolveAttachmentKind(input: {
    filename?: string;
    mime?: string;
    bytes?: Uint8Array;
}): AttachmentResolution {
    const ext = extension(input.filename);
    const declared = normalizedMime(input.mime);
    const bytes = input.bytes;

    return (
        resolveSignature(ext, declared, bytes) ??
        resolveMedia(ext, declared, bytes) ??
        resolveText(input.filename, ext, declared)
    );
}

function resolveSignature(ext: string, declared: string, bytes?: Uint8Array): AttachmentResolution | undefined {
    if (
        starts(bytes, [0x50, 0x4b, 0x03, 0x04]) ||
        archiveExtensions.has(ext) ||
        declared.includes("zip") ||
        declared.includes("compressed")
    )
        return { kind: "unsupported", reason: "archive", mime: declared || "application/octet-stream" };
    if (starts(bytes, [0x4d, 0x5a]) || binaryExtensions.has(ext))
        return { kind: "unsupported", reason: "binary", mime: declared || "application/octet-stream" };
    if (starts(bytes, [0x25, 0x50, 0x44, 0x46]) || ext === "pdf" || declared === "application/pdf")
        return { kind: "pdf", mime: "application/pdf" };
}

function resolveMedia(ext: string, declared: string, bytes?: Uint8Array): AttachmentResolution | undefined {
    if (
        starts(bytes, [0x89, 0x50, 0x4e, 0x47]) ||
        starts(bytes, [0xff, 0xd8, 0xff]) ||
        imageExtensions.has(ext) ||
        declared.startsWith("image/")
    )
        return { kind: "image", mime: declared.startsWith("image/") ? declared : mimeForExtension(ext, "image/png") };
    if (audioExtensions.has(ext) || declared.startsWith("audio/"))
        return { kind: "audio", mime: declared.startsWith("audio/") ? declared : mimeForExtension(ext, "audio/mpeg") };
    if (videoExtensions.has(ext) || declared.startsWith("video/"))
        return { kind: "video", mime: declared.startsWith("video/") ? declared : mimeForExtension(ext, "video/mp4") };
}

function resolveText(filename: string | undefined, ext: string, declared: string): AttachmentResolution {
    const basename = attachmentBasename(filename);

    if (csvExtensions.has(ext) || declared === "text/csv" || declared === "text/tab-separated-values")
        return {
            kind: "csv",
            mime: ext === "tsv" || declared === "text/tab-separated-values" ? "text/tab-separated-values" : "text/csv",
        };
    if (
        textExtensions.has(ext) ||
        textBasenames.has(basename) ||
        declared.startsWith("text/") ||
        textualApplicationMimes.has(declared)
    )
        return { kind: "text", mime: declared && declared !== "application/octet-stream" ? declared : "text/plain" };
    return { kind: "unsupported", reason: "binary", mime: declared || "application/octet-stream" };
}

function attachmentBasename(filename?: string) {
    return filename?.replaceAll("\\", "/").split("/").at(-1)?.toLowerCase() ?? "";
}

function mimeForExtension(ext: string, fallback: string) {
    const values: Record<string, string> = {
        avif: "image/avif",
        gif: "image/gif",
        jpeg: "image/jpeg",
        jpg: "image/jpeg",
        svg: "image/svg+xml",
        webp: "image/webp",
        wav: "audio/wav",
        ogg: "audio/ogg",
        webm: fallback.startsWith("audio/") ? "audio/webm" : "video/webm",
        mov: "video/quicktime",
    };
    return values[ext] ?? fallback;
}

function attachmentLanguage(filename: string) {
    const ext = extension(filename);
    const aliases: Record<string, string> = {
        cc: "cpp",
        conf: "config",
        env: "dotenv",
        h: "c",
        hpp: "cpp",
        htm: "html",
        js: "javascript",
        jsx: "javascript",
        md: "markdown",
        mjs: "javascript",
        py: "python",
        rb: "ruby",
        rs: "rust",
        sh: "shell",
        ts: "typescript",
        tsx: "typescript",
        yml: "yaml",
    };
    return aliases[ext] ?? (ext || "text");
}

export function decodeUtf8(data: ArrayBuffer | Uint8Array) {
    const bytes = data instanceof Uint8Array ? data : new Uint8Array(data);
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes).replace(/^\uFEFF/, "");
}

function lineCount(text: string) {
    return text.length ? text.split(/\r\n?|\n/).length : 0;
}

function textPreview(text: string) {
    const value = text.trim().slice(0, 700);
    return value.length < text.trim().length ? `${value}\n...` : value;
}

function formatTextAttachment(filename: string, language: string, content: string) {
    const truncated = content.length > maxTextAttachmentChars;
    const body = truncated ? content.slice(0, maxTextAttachmentChars) : content;
    const note = truncated ? `; truncated to ${maxTextAttachmentChars.toLocaleString("en-US")} characters` : "";
    return {
        text: `[Attachment: ${filename} (${language}${note})]\n\n\`\`\`${language}\n${body}\n\`\`\``,
        truncated,
    };
}

export function collectPdfText(pages: string[], totalPages: number) {
    let text = "";
    let extractedPages = 0;
    let truncated = totalPages > pages.length;
    for (const page of pages) {
        const heading = `--- Page ${extractedPages + 1} ---\n`;
        const prefix = text ? "\n\n" : "";
        const remaining = maxPdfExtractionChars - text.length - prefix.length - heading.length;
        if (remaining <= 0) {
            truncated = true;
            break;
        }
        text += `${prefix}${heading}${page.slice(0, remaining)}`;
        extractedPages++;
        if (page.length > remaining) {
            truncated = true;
            break;
        }
    }
    return { text, extractedPages, truncated };
}

export function formatPdfAttachment(filename: string, pages: number, collected: ReturnType<typeof collectPdfText>) {
    const note = collected.truncated ? `; extracted ${collected.extractedPages} pages; content truncated` : "";
    return `[Attachment: ${filename} (PDF, ${pages} pages${note})]\n\n${collected.text || "[No extractable text found]"}`;
}

export async function prepareAttachment(file: File, id: string = crypto.randomUUID()): Promise<AttachmentPreparation> {
    const initial = resolveAttachmentKind({ filename: file.name, mime: file.type });
    try {
        const signature = new Uint8Array(await file.slice(0, 16).arrayBuffer());
        const resolved = resolveAttachmentKind({ filename: file.name, mime: file.type, bytes: signature });
        if (resolved.kind === "unsupported") return { ok: false, reason: resolved.reason };
        if (file.size > attachmentSizeLimits[resolved.kind])
            return { ok: false, reason: "too-large", kind: resolved.kind, limit: attachmentSizeLimits[resolved.kind] };

        const base = { id, filename: file.name, mime: resolved.mime, size: file.size, status: "ready" as const };
        if (resolved.kind === "image" || resolved.kind === "audio" || resolved.kind === "video") {
            return { ok: true, attachment: { ...base, dataUrl: await readDataUrl(file), meta: {} } };
        }

        const data = await file.arrayBuffer();
        if (resolved.kind === "pdf") {
            const { extractPdfAttachment } = await import("./pdf-attachment");
            const extracted = await extractPdfAttachment(data, file.name);
            return {
                ok: true,
                attachment: {
                    ...base,
                    text: extracted.text,
                    meta: {
                        pages: extracted.pages,
                        extractedPages: extracted.extractedPages,
                        preview: textPreview(extracted.text),
                        thumbnail: extracted.thumbnail,
                        truncated: extracted.truncated,
                    },
                },
            };
        }

        let content: string;
        try {
            content = decodeUtf8(data);
        } catch {
            return { ok: false, reason: "invalid-utf8", kind: resolved.kind };
        }
        if (resolved.kind === "csv") {
            const parsed = parseCsvAttachment(content);
            return {
                ok: true,
                attachment: {
                    ...base,
                    text: formatCsvAttachment(file.name, parsed),
                    meta: {
                        rows: parsed.rows,
                        columns: parsed.columns,
                        preview: parsed.preview,
                        truncated: parsed.truncated,
                    },
                },
            };
        }
        const language = attachmentLanguage(file.name);
        const formatted = formatTextAttachment(file.name, language, content);
        return {
            ok: true,
            attachment: {
                ...base,
                text: formatted.text,
                meta: {
                    lines: lineCount(content),
                    language,
                    preview: textPreview(content),
                    truncated: formatted.truncated,
                },
            },
        };
    } catch {
        return { ok: false, reason: "read-failed", kind: initial.kind === "unsupported" ? undefined : initial.kind };
    }
}

function readDataUrl(file: File) {
    return new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(String(reader.result));
        reader.onerror = () => reject(reader.error);
        reader.readAsDataURL(file);
    });
}

export type ModelInputCapabilities = {
    name?: string;
    capabilities?: { input?: Partial<Record<"text" | "audio" | "image" | "video" | "pdf", boolean>> };
};

export function unsupportedModelAttachment(
    attachments: Pick<StagedAttachment, "filename" | "mime">[],
    model: ModelInputCapabilities | undefined,
) {
    for (const attachment of attachments) {
        const resolved = resolveAttachmentKind(attachment);
        if (resolved.kind !== "audio" && resolved.kind !== "video") continue;
        if (model?.capabilities?.input?.[resolved.kind] === true) continue;
        return { attachment, kind: resolved.kind };
    }
}

export async function prepareAttachmentsForSend(attachments: StagedAttachment[]) {
    const readable: string[] = [];
    const files: { filename: string; mime: string; url: string }[] = [];
    for (let attachment of attachments) {
        const resolved = resolveAttachmentKind(attachment);
        if (resolved.kind === "unsupported" || attachment.status !== "ready") continue;
        if (resolved.kind === "text" || resolved.kind === "csv" || resolved.kind === "pdf") {
            if (!attachment.text && attachment.dataUrl) {
                const legacy = dataUrlFile(attachment.dataUrl, attachment.filename, resolved.mime);
                const prepared = legacy ? await prepareAttachment(legacy, attachment.id) : undefined;
                if (prepared?.ok) attachment = prepared.attachment;
            }
            if (attachment.text) readable.push(attachment.text);
            continue;
        }
        if (attachment.dataUrl)
            files.push({ filename: attachment.filename, mime: resolved.mime, url: attachment.dataUrl });
    }
    return { text: readable.join("\n\n"), files };
}

function dataUrlFile(url: string, filename: string, mime: string) {
    const match = url.match(/^data:([^;,]*)(;base64)?,(.*)$/s);
    if (!match) return;
    try {
        const raw = match[2] ? atob(match[3]) : decodeURIComponent(match[3]);
        const bytes = Uint8Array.from(raw, (character) => character.charCodeAt(0));
        return new File([bytes], filename, { type: match[1] || mime });
    } catch {
        return;
    }
}

export function formatAttachmentBytes(bytes: number) {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < mib) return `${Math.ceil(bytes / 1024)} KB`;
    return `${Math.ceil(bytes / mib)} MB`;
}
