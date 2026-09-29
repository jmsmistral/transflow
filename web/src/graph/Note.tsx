/** Restricted Markdown: emphasis, strong and inline code only. HTML, links and images stay text. */
export function NoteText({
  text,
  markdown,
}: {
  text: string;
  markdown: boolean;
}) {
  if (!markdown) return <span className="note-text">{text}</span>;
  return (
    <span className="note-text">
      {text
        .split(/(\*\*[^*\n]+\*\*|\*[^*\n]+\*|`[^`\n]+`)/u)
        .map((part, i) =>
          part.startsWith("**") && part.endsWith("**") ? (
            <strong key={i}>{part.slice(2, -2)}</strong>
          ) : part.startsWith("*") && part.endsWith("*") ? (
            <em key={i}>{part.slice(1, -1)}</em>
          ) : part.startsWith("`") && part.endsWith("`") ? (
            <code key={i}>{part.slice(1, -1)}</code>
          ) : (
            part
          ),
        )}
    </span>
  );
}
