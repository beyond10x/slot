/**
 * The host components of the UI spec (spec/ui/slot.ui.yaml), one per host slot, by its name. The
 * generated pages draw a slot with the component registered here; `HostName` comes from the spec,
 * so a slot left out or a name the spec does not declare fails the type check. The names are the
 * lines of spec/ui/slot.hosts, which `ess ui check --hosts` checks.
 */
import type { Component } from 'vue';
import type { HostName } from './generated/ui.ts';
import JobOutput from './components/hosts/JobOutput.vue';
import JobRepo from './components/hosts/JobRepo.vue';
import JobRunTime from './components/hosts/JobRunTime.vue';
import JobTool from './components/hosts/JobTool.vue';
import JobWait from './components/hosts/JobWait.vue';

// `ess-ui/1` takes each host name once in a document, so one component stands under one name per
// table it draws in.
export const hosts: Record<HostName, Component> = {
  JobOutput,
  RunningTool: JobTool,
  QueuedTool: JobTool,
  DoneTool: JobTool,
  RunningRepo: JobRepo,
  QueuedRepo: JobRepo,
  DoneRepo: JobRepo,
  RunningWait: JobWait,
  QueuedWait: JobWait,
  DoneWait: JobWait,
  RunningRunTime: JobRunTime,
  DoneRunTime: JobRunTime,
};
