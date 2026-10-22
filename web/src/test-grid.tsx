import {
  createElement,
  forwardRef,
  useEffect,
  useImperativeHandle,
  useRef,
  type ReactNode,
} from "react";
import type {
  ViewPortScrollEvent,
  ColumnTemplateFunc,
  ColumnTemplateProp,
  VNode,
  ColumnRegular,
} from "@revolist/revogrid";

// Render our native header template without starting Stencil's component runtime.
const headerNode: Parameters<ColumnTemplateFunc>[0] = (
  tag: unknown,
  attrs?: unknown,
  children?: unknown,
): VNode => {
  if (typeof tag !== "string")
    throw new Error("Test header requires an HTML tag");
  return {
    $flags$: 0,
    $tag$: tag,
    $elm$: null,
    $text$: typeof children === "string" ? children : "",
    $attrs$: attrs,
    $children$: Array.isArray(children) ? children : [],
  };
};
function headerContent(node: VNode): ReactNode {
  if (typeof node.$tag$ !== "string")
    throw new Error("Unsupported test header");
  const attrs = node.$attrs$ as Record<string, unknown>;
  const { class: className, ...props } = attrs ?? {};
  return createElement(
    node.$tag$,
    { ...props, className },
    node.$text$ ||
      node.$children$.map((child, index) =>
        createElement(
          "span",
          { key: index, style: { display: "contents" } },
          headerContent(child),
        ),
      ),
  );
}

/** Test-only adapter: no custom elements, layout workers or startup timers. */
export const RevoGrid = forwardRef<
  HTMLTableElement,
  {
    columns: {
      prop: string;
      name: string;
      size?: number;
      columnTemplate?: ColumnTemplateFunc;
    }[];
    source: Record<string, string>[];
    "aria-label"?: string;
    className?: string;
    onAftercolumnresize?: (
      event: CustomEvent<Record<number, ColumnRegular>>,
    ) => void;
    onViewportscroll?: (event: CustomEvent<ViewPortScrollEvent>) => void;
  }
>(function TestGrid(
  {
    columns,
    source,
    "aria-label": label,
    className,
    onViewportscroll,
    onAftercolumnresize,
  },
  ref,
) {
  const table = useRef<HTMLTableElement>(null);
  useImperativeHandle(ref, () => {
    const element = table.current;
    if (!element) throw new Error("Test grid has not mounted");
    return Object.assign(element, {
      getProviders: async () => ({
        viewport: { stores: { rgRow: { store: { get: () => 240 } } } },
        dimension: {
          stores: { rgRow: { store: { get: () => source.length * 24 } } },
        },
      }),
    });
  }, [source.length]);
  useEffect(() => {
    const element = table.current;
    const scroll = (event: Event) =>
      onViewportscroll?.(event as CustomEvent<ViewPortScrollEvent>);
    const resize = (event: Event) =>
      onAftercolumnresize?.(
        event as CustomEvent<Record<number, ColumnRegular>>,
      );
    element?.addEventListener("viewportscroll", scroll);
    element?.addEventListener("aftercolumnresize", resize);
    return () => {
      element?.removeEventListener("viewportscroll", scroll);
      element?.removeEventListener("aftercolumnresize", resize);
    };
  }, [onViewportscroll, onAftercolumnresize]);
  return (
    <table ref={table} aria-label={label} className={className}>
      <thead>
        <tr>
          <th>#</th>
          {columns.map((column) => (
            <th
              key={column.prop}
              aria-label={column.name}
              style={{ width: column.size }}
            >
              {column.columnTemplate
                ? headerContent(
                    column.columnTemplate(
                      headerNode,
                      column as ColumnTemplateProp,
                    ) as VNode,
                  )
                : column.name}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {source.map((row, index) => (
          <tr key={index}>
            <th>{index + 1}</th>
            {columns.map((column) => (
              <td key={column.prop}>{row[column.prop]}</td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
});
