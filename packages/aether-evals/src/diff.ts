export interface GitDiff {
  diff: string;
  stats: DiffStats;
}

export interface DiffStats {
  filesChanged: number;
  linesAdded: number;
  linesRemoved: number;
}

export function diffStatsFromDiff(diff: string): DiffStats {
  let filesChanged = 0;
  let linesAdded = 0;
  let linesRemoved = 0;

  for (const line of diff.split("\n")) {
    if (line.startsWith("diff --git")) {
      filesChanged += 1;
    } else if (line.startsWith("+") && !line.startsWith("+++")) {
      linesAdded += 1;
    } else if (line.startsWith("-") && !line.startsWith("---")) {
      linesRemoved += 1;
    }
  }

  return { filesChanged, linesAdded, linesRemoved };
}
