import { LeftRail } from "@/components/LeftRail";
import { MissionBar } from "@/components/MissionBar";
import { LeftPanel } from "@/components/LeftPanel";
import { CenterPane } from "@/components/CenterPane";
import { Composer } from "@/components/Composer";
import { Inspector } from "@/components/Inspector";
import { useViewStore } from "@/store/viewStore";
import { cn } from "@/lib/utils";

export function Layout() {
  const leftCollapsed = useViewStore((s) => s.leftCollapsed);
  const rightCollapsed = useViewStore((s) => s.rightCollapsed);

  return (
    <div
      className={cn(
        "grid h-screen w-screen overflow-hidden",
        "grid-rows-[var(--quorp-mission-bar)_1fr]",
      )}
      style={{
        gridTemplateColumns:
          `var(--quorp-rail-width) ` +
          `${leftCollapsed ? "0" : "minmax(280px, 340px)"} 1fr ` +
          `${rightCollapsed ? "0" : "minmax(360px, 420px)"}`,
        // Custom prop fallbacks (Tailwind already maps them, but the
        // grid-template-columns string above resolves to literal CSS
        // so we expose the rail width as a custom prop here too).
        ["--quorp-rail-width" as string]: "56px",
      }}
    >
      <div className="col-span-full">
        <MissionBar />
      </div>
      <LeftRail />
      <LeftPanel className={cn(leftCollapsed && "hidden")} />
      <main className="flex flex-col min-w-0 border-l border-border-subtle">
        <CenterPane />
        <Composer />
      </main>
      <Inspector className={cn(rightCollapsed && "hidden")} />
    </div>
  );
}
