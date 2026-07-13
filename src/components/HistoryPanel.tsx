import { ChevronLeft, ChevronRight } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import type { UsageHistoryEntry } from "../types";
import { formatDateTime } from "../lib/format";

interface HistoryPanelProps {
  items: UsageHistoryEntry[];
}

export function HistoryPanel({ items }: HistoryPanelProps) {
  const pageSize = 15;
  const pageCount = Math.max(1, Math.min(10, Math.ceil(items.length / pageSize)));
  const [currentPage, setCurrentPage] = useState(1);
  const newestId = items[0]?.id;

  useEffect(() => {
    setCurrentPage(1);
  }, [newestId]);

  const pageItems = useMemo(() => {
    const start = (currentPage - 1) * pageSize;
    return items.slice(start, start + pageSize);
  }, [currentPage, items]);

  return (
    <section className="rounded-lg border border-border bg-panel p-5 shadow-soft">
      <div className="mb-4 flex items-center justify-between gap-3">
        <div>
          <h2 className="m-0 text-lg font-semibold text-ink">Recent history</h2>
          <p className="m-0 mt-1 text-sm text-slate-400">
            Latest quota snapshots stored locally.
          </p>
        </div>
      </div>

      <div className="overflow-x-auto rounded-lg border border-slate-800">
        <table className="min-w-full border-collapse">
          <thead className="bg-slate-950/80">
            <tr className="text-left text-xs uppercase tracking-wide text-slate-500">
              <th className="px-4 py-3 font-medium">Provider</th>
              <th className="px-4 py-3 font-medium">Limit</th>
              <th className="px-4 py-3 font-medium">Usage</th>
              <th className="px-4 py-3 font-medium">Percent</th>
              <th className="px-4 py-3 font-medium">Recorded</th>
            </tr>
          </thead>
          <tbody>
            {pageItems.map((item) => (
              <tr key={item.id} className="border-t border-slate-800 text-sm text-slate-200">
                <td className="px-4 py-3 capitalize">{item.provider}</td>
                <td className="px-4 py-3">{item.limitLabel}</td>
                <td className="px-4 py-3">
                  {item.usedValue} / {item.totalValue}
                </td>
                <td className="px-4 py-3">{item.percentage}%</td>
                <td className="px-4 py-3 text-slate-400">{formatDateTime(item.recordedAt)}</td>
              </tr>
            ))}
            {pageItems.length === 0 ? (
              <tr className="border-t border-slate-800 text-sm text-slate-500">
                <td colSpan={5} className="px-4 py-8 text-center">
                  No history on this page yet
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>

      <nav className="mt-4 flex flex-wrap items-center justify-center gap-1" aria-label="History pages">
        <button
          type="button"
          title="Previous page"
          aria-label="Previous history page"
          disabled={currentPage === 1}
          onClick={() => setCurrentPage((page) => Math.max(1, page - 1))}
          className="inline-flex h-8 w-8 items-center justify-center rounded-md border border-slate-700 text-slate-400 transition hover:bg-slate-800 hover:text-white disabled:cursor-not-allowed disabled:opacity-35"
        >
          <ChevronLeft className="h-4 w-4" />
        </button>
        {Array.from({ length: pageCount }, (_, index) => index + 1).map((page) => (
          <button
            key={page}
            type="button"
            aria-label={`History page ${page}`}
            aria-current={currentPage === page ? "page" : undefined}
            onClick={() => setCurrentPage(page)}
            className={`inline-flex h-8 min-w-8 items-center justify-center rounded-md border px-2 text-xs font-medium transition ${
              currentPage === page
                ? "border-slate-500 bg-slate-700 text-white"
                : "border-slate-700 text-slate-400 hover:bg-slate-800 hover:text-white"
            }`}
          >
            {page}
          </button>
        ))}
        <button
          type="button"
          title="Next page"
          aria-label="Next history page"
          disabled={currentPage === pageCount}
          onClick={() => setCurrentPage((page) => Math.min(pageCount, page + 1))}
          className="inline-flex h-8 w-8 items-center justify-center rounded-md border border-slate-700 text-slate-400 transition hover:bg-slate-800 hover:text-white disabled:cursor-not-allowed disabled:opacity-35"
        >
          <ChevronRight className="h-4 w-4" />
        </button>
      </nav>
    </section>
  );
}
