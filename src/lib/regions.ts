import { invoke } from "@tauri-apps/api/core";

export type RegionKey = "shake" | "fish_bar" | "shake_template";

export type Region = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type RegionsConfig = {
  screen_width: number;
  screen_height: number;
  shake: Region | null;
  fish_bar: Region | null;
  shake_template: Region | null;
};

export const REGION_META: Record<RegionKey, { label: string; color: string; defaultSize: { w: number; h: number } }> = {
  shake: {
    label: "Shake",
    color: "#ff4d4d",
    defaultSize: { w: 600, h: 400 },
  },
  fish_bar: {
    label: "Fish Bar",
    color: "#3b82f6",
    defaultSize: { w: 800, h: 60 },
  },
  shake_template: {
    label: "Template",
    color: "#a855f7",
    defaultSize: { w: 200, h: 200 },
  },
};

export async function loadRegions(): Promise<RegionsConfig> {
  return await invoke<RegionsConfig>("load_regions");
}

export async function saveRegions(config: RegionsConfig): Promise<void> {
  await invoke("save_regions", { config });
}

export async function getScreenSize(): Promise<[number, number]> {
  return await invoke<[number, number]>("get_screen_size");
}
