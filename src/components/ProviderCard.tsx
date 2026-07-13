import { GripVertical, RefreshCw, TriangleAlert } from "lucide-react";
import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import type { ProviderSnapshot } from "../types";
import { formatDateTime, formatPercent } from "../lib/format";
import { ProviderIcon } from "./ProviderIcon";

interface ProviderCardProps {
  provider: ProviderSnapshot;
  refreshing: boolean;
  twoColumnQuota?: boolean;
  onRefresh: () => void;
}

function ProgressBar({
  used,
  total,
}: {
  used: number;
  total: number;
}) {
  const ratio = total <= 0 ? 0 : Math.min(100, Math.round((used / total) * 100));
  const hue = Math.round((1 - ratio / 100) * 125);
  const startHue = Math.min(125, hue + 16);

  return (
    <div className="h-2.5 w-full overflow-hidden rounded-full bg-slate-800" title={`${ratio}% used`}>
      <div
        className="h-full rounded-full transition-all duration-500"
        style={{
          width: `${ratio}%`,
          background: `linear-gradient(90deg, hsl(${startHue} 78% 48%), hsl(${hue} 82% 55%))`,
          boxShadow: `0 0 12px hsl(${hue} 78% 48% / 0.3)`
        }}
      />
    </div>
  );
}

function StatusBadge({ provider }: { provider: ProviderSnapshot }) {
  const map = {
    ready: "Ready",
    needs_auth: "Needs auth",
    needs_config: "Needs config",
    error: "Error",
    placeholder: "Placeholder"
  } as const;

  return (
    <span
      className="cursor-help rounded-full border border-slate-700 px-2.5 py-1 text-xs text-slate-300"
      title={provider.notes}
      tabIndex={0}
    >
      {map[provider.status]}
    </span>
  );
}

export function ProviderCard({
  provider,
  refreshing,
  twoColumnQuota = false,
  onRefresh
}: ProviderCardProps) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: provider.accountId
  });
  const isWarning = provider.limits.some(
    (limit) => limit.total > 0 && limit.used / limit.total >= 0.8
  );

  return (
    <article
      ref={setNodeRef}
      style={{ transform: CSS.Transform.toString(transform), transition, zIndex: isDragging ? 20 : undefined }}
      className={`relative flex h-full flex-col gap-5 rounded-lg border border-border bg-panel p-5 shadow-soft ${isDragging ? "opacity-70 shadow-2xl" : ""}`}
    >
      <button
        type="button"
        title="Drag to reorder"
        aria-label={`Drag ${provider.providerName} to reorder`}
        {...attributes}
        {...listeners}
        className="absolute left-1 top-1 inline-flex h-5 w-5 touch-none items-center justify-center cursor-grab rounded text-slate-500 hover:bg-slate-800 hover:text-white active:cursor-grabbing"
      >
        <GripVertical className="h-3.5 w-3.5" />
      </button>
      <div className="flex items-start justify-between gap-4">
        <div className="flex items-center gap-3">
          <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-lg border border-slate-700 bg-slate-950 text-white">
            <div className="h-7 w-7 [&>svg]:h-full [&>svg]:w-full">
              <ProviderIcon provider={provider.provider} />
            </div>
          </div>
          <div>
            <div className="flex items-center gap-2">
              <h2 className="m-0 text-lg font-semibold text-ink">{provider.providerName}</h2>
              {isWarning ? <TriangleAlert className="h-4 w-4 text-amber-400" /> : null}
              <button
                type="button"
                onClick={onRefresh}
                disabled={refreshing}
                title={`Refresh ${provider.providerName}`}
                aria-label={`Refresh ${provider.providerName}`}
                className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-slate-400 transition hover:bg-slate-800 hover:text-white disabled:cursor-not-allowed disabled:opacity-50"
              >
                <RefreshCw className={`h-4 w-4 ${refreshing ? "animate-spin" : ""}`} />
              </button>
            </div>
            <p className="m-0 mt-1 text-sm text-slate-400">
              {provider.customName ? `${provider.customName} - ${provider.planName}` : provider.planName}
            </p>
          </div>
        </div>
        <StatusBadge provider={provider} />
      </div>

      <div className={twoColumnQuota ? "grid grid-cols-2 gap-x-4 gap-y-4" : "space-y-4"}>
        {provider.limits.length > 0 ? provider.limits.map((limit) => (
          <section key={limit.id} className="space-y-2">
            <div className="flex items-start justify-between gap-2">
              <span className="min-w-0 text-sm leading-snug text-slate-300">{limit.label}</span>
              <span className="shrink-0 whitespace-nowrap text-sm font-medium text-ink">
                {formatPercent(limit.used, limit.total)} ({limit.used}/{limit.total})
              </span>
            </div>
            <ProgressBar used={limit.used} total={limit.total} />
            <p className="m-0 text-xs text-slate-500">
              {limit.resetAt ? `Resets ${formatDateTime(limit.resetAt)}` : "Reset time unavailable"}
            </p>
          </section>
        )) : (
          <div className="rounded-lg border border-dashed border-slate-700 px-3 py-6 text-center text-sm text-slate-500">
            No quota windows yet
          </div>
        )}
      </div>

    </article>
  );
}
