import { create } from 'zustand';

interface PaletteState {
  open: boolean;
  openPalette: () => void;
  closePalette: () => void;
  togglePalette: () => void;
}

export const usePaletteStore = create<PaletteState>((set) => ({
  open: false,
  openPalette: () => set({ open: true }),
  closePalette: () => set({ open: false }),
  togglePalette: () => set((state) => ({ open: !state.open })),
}));

export function openCommandPalette(): void {
  usePaletteStore.getState().openPalette();
}
