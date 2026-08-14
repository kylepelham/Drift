# Attachments

Drift resolves attachments from their filename, declared MIME type, and common file signatures before staging them. File picker, drag-and-drop, and clipboard files all use the same path.

## Supported types

- Images remain native image prompt parts and keep thumbnail/lightbox rendering.
- Audio and video remain native file parts with playback rendering. Drift rejects them before send unless the selected model advertises the corresponding input capability.
- UTF-8 text, source, and configuration files are sent as readable prompt text with a filename and language header. Their chips show line counts and a hover preview.
- CSV and TSV files are delimiter-sniffed and sent as a bounded Markdown table with row and column counts.
- PDFs are parsed in a PDF.js worker. Drift extracts up to 12 pages and 100,000 characters, renders the first page as the staging thumbnail, and tells the model when extraction was truncated.

Archives, executables, databases, opaque binaries, and legacy/binary spreadsheet formats are rejected before staging. Limits are type-specific: 2 MiB for text, 5 MiB for CSV/TSV, 10 MiB for images, 20 MiB for PDFs and audio, and 40 MiB for video.

Text, CSV, and PDF drafts retain only bounded readable content and metadata, not base64 copies of their source files. Native media remains encoded as a data URL until the prompt is admitted, after which the draft is cleared.
