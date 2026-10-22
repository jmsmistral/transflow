import { Icon } from "./Icons";
import type { PreviewCriteria } from "./preview-query";

export function PreviewCriteriaPills({
  criteria,
}: {
  criteria: PreviewCriteria;
}) {
  return (
    <div
      className="preview-criteria-pills"
      aria-label="Applied filters and sorts"
    >
      {criteria.filters.map((filter) => {
        const description = `${filter.column}: ${filter.kind === "null" ? "Only nulls" : filter.kind === "not_null" ? "Exclude nulls" : `${filter.kind === "include" ? "Include only" : "Exclude"} ${JSON.stringify("value" in filter ? filter.value : "")}`}`;
        return (
          <span
            className="preview-criteria-pill"
            key={`filter:${filter.column}`}
            title={description}
            aria-label={description}
          >
            <Icon
              name={
                filter.kind === "exclude" || filter.kind === "not_null"
                  ? "filterExclude"
                  : "filterInclude"
              }
            />
            <span>{filter.column}</span>
          </span>
        );
      })}
      {criteria.sorts.map((sort, index) => {
        const description = `${sort.column}: Sort ${sort.direction === "asc" ? "ascending" : "descending"}${criteria.sorts.length > 1 ? ` (priority ${index + 1})` : ""}`;
        return (
          <span
            className="preview-criteria-pill"
            key={`sort:${sort.column}`}
            title={description}
            aria-label={description}
          >
            <Icon
              name={
                sort.direction === "asc" ? "sortAscending" : "sortDescending"
              }
            />
            <span>{sort.column}</span>
            {criteria.sorts.length > 1 && <small>{index + 1}</small>}
          </span>
        );
      })}
    </div>
  );
}
