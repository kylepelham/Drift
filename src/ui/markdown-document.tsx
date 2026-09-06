import { createEffect, onCleanup } from "solid-js"
import { previewParentDirectory } from "../file-preview"
import { shouldPreviewFile } from "../state/file-preview-prefs"
import { Markdown } from "./markdown"
import { observeMarkdownImages } from "./markdown-images"

export function MarkdownDocument(props: { text: string; path: string; directory: string; hash?: string }) {
  let root!: HTMLDivElement
  createEffect(() => {
    void props.text
    onCleanup(observeMarkdownImages(root, {
      parent: previewParentDirectory(props.path),
      directory: props.directory,
      enabled: shouldPreviewFile("image.png"),
      hash: props.hash,
    }))
  })

  return (
    <div ref={root}>
      <Markdown text={props.text} directory={previewParentDirectory(props.path)} workspaceDirectory={props.directory} documentPreview done />
    </div>
  )
}
