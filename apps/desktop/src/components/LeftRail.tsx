import {
  Folder,
  Clock,
  CheckSquare,
  Book,
  Database,
  FlaskConical,
  Stethoscope,
  Settings as SettingsIcon,
} from "lucide-react";

import { cn } from "@/lib/utils";
import { useViewStore, type Surface } from "@/store/viewStore";

const RAIL_ITEMS: { surface: Surface; label: string; icon: typeof Folder }[] = [
  { surface: "workspaces", label: "Workspaces", icon: Folder },
  { surface: "sessions", label: "Sessions", icon: Clock },
  { surface: "tasks", label: "Tasks", icon: CheckSquare },
  { surface: "rules", label: "Rules", icon: Book },
  { surface: "memory", label: "Memory", icon: Database },
  { surface: "benchmarks", label: "Benchmarks", icon: FlaskConical },
  { surface: "doctor", label: "Doctor", icon: Stethoscope },
];

export function LeftRail() {
  const surface = useViewStore((s) => s.surface);
  const setSurface = useViewStore((s) => s.setSurface);

  return (
    <nav
      aria-label="Primary navigation"
      className="flex h-full w-rail flex-col items-center gap-1 border-r border-border-subtle bg-bg-surface py-2"
    >
      {RAIL_ITEMS.map(({ surface: s, label, icon: Icon }) => (
        <button
          key={s}
          type="button"
          aria-label={label}
          title={label}
          onClick={() => setSurface(s)}
          className={cn(
            "flex h-10 w-10 items-center justify-center rounded-md text-fg-muted transition-colors",
            "hover:bg-bg-elevated hover:text-fg-primary",
            surface === s &&
              "bg-bg-elevated text-fg-primary outline outline-2 outline-ring-focus",
          )}
        >
          <Icon size={18} />
        </button>
      ))}
      <div className="flex-1" />
      <button
        type="button"
        aria-label="Settings"
        title="Settings"
        className="mb-2 flex h-10 w-10 items-center justify-center rounded-md text-fg-muted hover:bg-bg-elevated hover:text-fg-primary"
      >
        <SettingsIcon size={18} />
      </button>
    </nav>
  );
}
