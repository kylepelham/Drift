import { responseRevealSegmentSize } from "./response-animation";

type MarkdownAddition = Text | HTMLElement;

export function markdownNodeSignature(node: Node) {
    return node.nodeType === Node.ELEMENT_NODE
        ? (node as Element).outerHTML
        : `${node.nodeType}:${node.textContent ?? ""}`;
}

function collectWholeAddition(node: Node, additions: MarkdownAddition[]) {
    if (node.nodeType === Node.TEXT_NODE) {
        // Wrapping Marked's table whitespace in spans would create anonymous table cells.
        if (node.textContent?.trim()) additions.push(node as Text);
        return;
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return;
    if (!node.textContent) {
        additions.push(node as HTMLElement);
        return;
    }

    for (const child of [...node.childNodes]) collectWholeAddition(child, additions);
}

function markMarkdownAddition(previous: Node | undefined, next: Node, additions: MarkdownAddition[]) {
    if (!previous || previous.nodeType !== next.nodeType) return collectWholeAddition(next, additions);

    if (next.nodeType === Node.TEXT_NODE) {
        const priorText = previous.textContent ?? "";
        const nextText = next.textContent ?? "";
        if (nextText === priorText || !nextText.startsWith(priorText) || !next.parentNode) return;

        const suffix = document.createTextNode(nextText.slice(priorText.length));
        next.parentNode.insertBefore(document.createTextNode(priorText), next);
        next.parentNode.replaceChild(suffix, next);
        additions.push(suffix);
        return;
    }
    if (next.nodeType !== Node.ELEMENT_NODE || (previous as Element).tagName !== (next as Element).tagName) return;

    const previousChildren = [...previous.childNodes];
    const nextChildren = [...next.childNodes];
    for (let index = 0; index < nextChildren.length; index++) {
        markMarkdownAddition(previousChildren[index], nextChildren[index], additions);
    }
}

function createTypingRevealNodes(additions: MarkdownAddition[]) {
    const revealedCharacters = additions.reduce(
        (total, addition) =>
            total + (addition.nodeType === Node.TEXT_NODE ? Array.from((addition as Text).data).length : 1),
        0,
    );
    const segmentSize = responseRevealSegmentSize(revealedCharacters);
    const revealNodes: HTMLElement[] = [];

    for (const addition of additions) {
        if (addition.nodeType === Node.ELEMENT_NODE) {
            revealNodes.push(addition as HTMLElement);
            continue;
        }
        if (!addition.parentNode) continue;

        const characters = Array.from((addition as Text).data);
        const fragment = document.createDocumentFragment();
        for (let index = 0; index < characters.length; index += segmentSize) {
            const span = document.createElement("span");
            span.textContent = characters.slice(index, index + segmentSize).join("");
            fragment.append(span);
            revealNodes.push(span);
        }
        addition.parentNode.replaceChild(fragment, addition);
    }

    return { revealNodes, revealedCharacters };
}

export function replaceMarkdownSuffix(
    root: HTMLElement,
    source: string,
    previousSignatures: string[],
    previousNodes: ChildNode[],
    reveal: boolean,
) {
    const template = document.createElement("template");
    template.innerHTML = source;
    const nodes = [...template.content.childNodes];
    const signatures = nodes.map(markdownNodeSignature);

    let unchanged = root.childNodes.length === previousSignatures.length ? 0 : -1;
    while (
        unchanged >= 0 &&
        unchanged < previousSignatures.length &&
        previousSignatures[unchanged] === signatures[unchanged]
    )
        unchanged++;
    if (unchanged < 0) unchanged = 0;

    while (root.childNodes.length > unchanged) root.lastChild?.remove();
    const fragment = document.createDocumentFragment();
    const renderedNodes = nodes.slice(unchanged).map((node) => node.cloneNode(true));
    fragment.append(...renderedNodes);

    const additions: MarkdownAddition[] = [];
    if (reveal) {
        for (let index = 0; index < renderedNodes.length; index++) {
            markMarkdownAddition(previousNodes[unchanged + index], renderedNodes[index], additions);
        }
    }
    const typing = createTypingRevealNodes(additions);
    root.append(fragment);

    return { signatures, nodes, ...typing };
}
