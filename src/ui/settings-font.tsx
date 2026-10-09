export function FontField(props: { label: string; value: string; onInput: (value: string) => void; mono?: boolean }) {
    return (
        <input
            aria-label={props.label}
            class="h-8 w-full rounded-md border border-edge bg-raised/45 px-2.5 text-sm text-ink outline-none placeholder:text-ink-faint focus:border-accent sm:w-56"
            classList={{ "font-mono text-xs": props.mono }}
            placeholder={props.mono ? '"Cascadia Code", monospace' : '"Segoe UI", sans-serif'}
            maxLength={256}
            value={props.value}
            onInput={(event) => props.onInput(event.currentTarget.value)}
        />
    );
}
