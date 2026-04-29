import { BenchmarksPanel } from "@/components/BenchmarksPanel";
import { DoctorPanel } from "@/components/DoctorPanel";
import { Timeline } from "@/components/Timeline";
import { useViewStore } from "@/store/viewStore";

/**
 * Switches between full-width center surfaces (Benchmarks, Doctor)
 * and the default Timeline view based on the left-rail selection.
 */
export function CenterPane() {
  const surface = useViewStore((s) => s.surface);

  if (surface === "benchmarks") return <BenchmarksPanel />;
  if (surface === "doctor") return <DoctorPanel />;
  return <Timeline />;
}
