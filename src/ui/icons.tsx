import type { JSX } from "solid-js"

type IconProps = { class?: string }

function frame(props: IconProps, children: JSX.Element) {
  return (
    <svg
      class={props.class ?? "size-4"}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.8"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      {children}
    </svg>
  )
}

export function IconSquarePen(props: IconProps) {
  return frame(
    props,
    <>
      <path d="M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" />
      <path d="M18.4 2.6a2.12 2.12 0 1 1 3 3L12 15l-4 1 1-4Z" />
    </>,
  )
}

export function IconDots(props: IconProps) {
  return frame(
    props,
    <>
      <circle cx="5" cy="12" r="0.8" fill="currentColor" />
      <circle cx="12" cy="12" r="0.8" fill="currentColor" />
      <circle cx="19" cy="12" r="0.8" fill="currentColor" />
    </>,
  )
}

export function IconArchive(props: IconProps) {
  return frame(
    props,
    <>
      <rect x="2" y="3" width="20" height="5" rx="1" />
      <path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8" />
      <path d="M10 12h4" />
    </>,
  )
}

export function IconGear(props: IconProps) {
  return frame(
    props,
    <>
      <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
      <circle cx="12" cy="12" r="3" />
    </>,
  )
}

export function IconSliders(props: IconProps) {
  return frame(
    props,
    <>
      <path d="M4 7h7M15 7h5M4 17h3M11 17h9" />
      <circle cx="13" cy="7" r="2" />
      <circle cx="9" cy="17" r="2" />
    </>,
  )
}

export function IconPlus(props: IconProps) {
  return frame(
    props,
    <>
      <path d="M12 5v14" />
      <path d="M5 12h14" />
    </>,
  )
}

export function IconRestore(props: IconProps) {
  return frame(
    props,
    <>
      <path d="M3 12a9 9 0 1 0 3-6.7" />
      <path d="M3 4v5h5" />
    </>,
  )
}

export function IconUndo(props: IconProps) {
  return frame(
    props,
    <>
      <path d="M9 7 4 12l5 5" />
      <path d="M4 12h9a6 6 0 0 1 6 6" />
    </>,
  )
}

export function IconCopy(props: IconProps) {
  return frame(
    props,
    <>
      <rect x="8" y="8" width="12" height="12" rx="2" />
      <path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" />
    </>,
  )
}

export function IconBranch(props: IconProps) {
  return frame(
    props,
    <>
      <circle cx="6" cy="6" r="2.5" />
      <circle cx="18" cy="18" r="2.5" />
      <path d="M6 8.5v3a4 4 0 0 0 4 4h5.5" />
      <path d="m13 12.5 3 3-3 3" />
    </>,
  )
}

export function IconArrowUpRight(props: IconProps) {
  return frame(
    props,
    <>
      <path d="M7 17 17 7" />
      <path d="M8 7h9v9" />
    </>,
  )
}

export function IconArrowUp(props: IconProps) {
  return frame(
    props,
    <>
      <path d="M12 19V5" />
      <path d="m5 12 7-7 7 7" />
    </>,
  )
}

export function IconPaperclip(props: IconProps) {
  return frame(
    props,
    <path d="m21 12-8.5 8.5a5 5 0 0 1-7-7L14 5a3.5 3.5 0 0 1 5 5l-8.5 8.5a2 2 0 0 1-3-3L16 7" />,
  )
}

export function IconX(props: IconProps) {
  return frame(
    props,
    <>
      <path d="M18 6 6 18" />
      <path d="m6 6 12 12" />
    </>,
  )
}

export function IconCheck(props: IconProps) {
  return frame(props, <path d="M20 6 9 17l-5-5" />)
}

export function IconShieldCheck(props: IconProps) {
  return frame(
    props,
    <>
      <path d="M12 3 5 6v5c0 4.6 2.9 8 7 10 4.1-2 7-5.4 7-10V6Z" />
      <path d="m9 12 2 2 4-4" />
    </>,
  )
}

export function IconBell(props: IconProps) {
  return frame(
    props,
    <>
      <path d="M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9" />
      <path d="M10 21h4" />
    </>,
  )
}

export function IconKeyboard(props: IconProps) {
  return frame(
    props,
    <>
      <rect x="2" y="5" width="20" height="14" rx="2" />
      <path d="M6 9h.01M10 9h.01M14 9h.01M18 9h.01M6 13h.01M10 13h.01M14 13h.01M18 13h.01M7 16h10" />
    </>,
  )
}

export function IconPalette(props: IconProps) {
  return frame(
    props,
    <>
      <path d="M12 3a9 9 0 0 0 0 18h1.5a2 2 0 0 0 0-4H12a1.5 1.5 0 0 1 0-3h3a6 6 0 0 0 0-11Z" />
      <circle cx="7.5" cy="10" r=".8" fill="currentColor" />
      <circle cx="9.5" cy="6.5" r=".8" fill="currentColor" />
      <circle cx="14" cy="6" r=".8" fill="currentColor" />
    </>,
  )
}

export function IconChip(props: IconProps) {
  return frame(
    props,
    <>
      <rect x="6" y="6" width="12" height="12" rx="2" />
      <path d="M9 1v3M15 1v3M9 20v3M15 20v3M20 9h3M20 14h3M1 9h3M1 14h3" />
      <rect x="9" y="9" width="6" height="6" rx="1" />
    </>,
  )
}

export function IconCode(props: IconProps) {
  return frame(
    props,
    <>
      <path d="m8 9-3 3 3 3" />
      <path d="m16 9 3 3-3 3" />
      <path d="m14 5-4 14" />
    </>,
  )
}

export function IconInfo(props: IconProps) {
  return frame(
    props,
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v5" />
      <path d="M12 8h.01" />
    </>,
  )
}
