import { z } from 'zod';

export const ProcessStatusSchema = z.enum([
  'STARTING',
  'RUNNING',
  'EXITED',
  'STOPPED',
  'INTERRUPTED',
]);

export const ProcessRunModeSchema = z.enum(['PIPED', 'PTY']);

export const ProcessIdSchema = z.uuidv7();

export const StartProcessRequestSchema = z.object({
  processId: ProcessIdSchema,
  program: z.string(),
  args: z.array(z.string()),
  cwd: z.string().optional(),
  envAllowlist: z.array(z.string()).optional(),
  runMode: ProcessRunModeSchema,
  dataDir: z.string(),
});

export const StartProcessResponseSchema = z.object({
  processId: ProcessIdSchema,
  status: ProcessStatusSchema,
});

export const StopProcessRequestSchema = z.object({
  processId: ProcessIdSchema,
  gracefulTimeoutMs: z.number().int().nonnegative().optional(),
});

export const StopProcessResponseSchema = z.object({
  processId: ProcessIdSchema,
  status: ProcessStatusSchema,
});

export const ResumeProcessRequestSchema = z.object({
  processId: ProcessIdSchema,
});

export const ResumeProcessResponseSchema = z.object({
  processId: ProcessIdSchema,
  status: ProcessStatusSchema,
});

export const ListProcessesRequestSchema = z.object({});

export const ProcessResourceCountersSchema = z.object({
  durationMs: z.number().int().nonnegative().optional(),
  stdoutBytes: z.number().int().nonnegative(),
  stderrBytes: z.number().int().nonnegative(),
  cpuMs: z.number().int().nonnegative().optional(),
  peakMemoryBytes: z.number().int().nonnegative().optional(),
});

export const ProcessSummarySchema = z.object({
  processId: ProcessIdSchema,
  status: ProcessStatusSchema,
  startedAt: z.string().optional(),
  exitedAt: z.string().optional(),
  exitCode: z.number().int().nonnegative().optional(),
  resourceCounters: ProcessResourceCountersSchema,
  droppedOutputBytes: z.number().int().nonnegative(),
});

export const ListProcessesResponseSchema = z.array(ProcessSummarySchema);

export const ProcessOutputRequestSchema = z.object({
  processId: ProcessIdSchema,
  offset: z.number().int().nonnegative(),
});

export const ProcessOutputFrameSchema = z.object({
  seq: z.number().int().nonnegative(),
  stream: z.enum(['STDOUT', 'STDERR', 'PTY']),
  payload: z.string(),
});

export const ProcessOutputResponseSchema = z.object({
  frames: z.array(ProcessOutputFrameSchema),
  nextOffset: z.number().int().nonnegative(),
  truncated: z.boolean(),
});
