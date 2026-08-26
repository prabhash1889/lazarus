export interface CommandDefinition {
  /** Stable unique id, e.g. `nav.home`. */
  id: string;
  /** Human readable title shown in the palette and cheat sheet. */
  title: string;
  /** Palette group label; ungrouped commands render without a header. */
  section?: string;
  /** Extra search terms beyond the title. */
  keywords?: string[];
  /**
   * Shortcut binding such as `mod+k` or a chord like `g h`. Conflicts with a
   * live registration are rejected; see command-registry.
   */
  shortcut?: string;
  /** Availability predicate evaluated whenever the command set is read. */
  when?: () => boolean;
  run: () => void;
}

export interface RegisteredCommand extends CommandDefinition {
  /** Id of the command that won the shortcut when this one lost the conflict. */
  shortcutRejectedBy?: string;
}
