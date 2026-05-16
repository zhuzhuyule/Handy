import { useSettings } from "../../../../hooks/useSettings";
import { ModelChainSelector } from "../../../ui/ModelChainSelector";

/**
 * Dedicated model selector for cluster generation on the Summary page.
 * Persists to `AppSettings.selected_clustering_model` and overrides the
 * post-process model fallback used by `resolve_clustering_target` on the
 * backend.
 */
export function ClusteringModelSelector() {
  const { settings, updateModelChain } = useSettings();
  return (
    <ModelChainSelector
      chain={settings?.selected_clustering_model ?? null}
      onChange={(chain) => updateModelChain("selected_clustering_model", chain)}
      modelFilter={(m) => m.model_type === "text"}
      defaultStrategy="serial"
    />
  );
}
