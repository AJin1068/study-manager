// Mirror of src-tauri/src/commands/topic.rs. Everything here is local: no network, no model.

import { invoke } from '@tauri-apps/api/core';

export type TopicNotice = 'no_plan' | 'no_wiki' | 'no_pages';
export type TopicSource = 'assigned' | 'name_match' | 'weak';
export type TopicReason = 'empty' | 'low_accuracy' | 'stale' | 'hard' | 'review' | 'chosen';

export interface TopicPage {
  pageId: number;
  pageName: string;
  markdown: string;
}

export interface TodayTopic {
  planId: number | null;
  planName: string | null;
  date: string;
  source: TopicSource | null;
  /** `auto` or `manual` when the page was assigned. */
  assignedBy: string | null;
  reason: TopicReason | null;
  page: TopicPage | null;
  notice: TopicNotice | null;
}

export interface TopicAssignment {
  planId: number;
  planName: string;
  date: string;
  pageId: number;
  pageName: string;
  source: string;
  reason: string;
}

export interface AssignReport {
  assigned: TopicAssignment[];
  /** Occurrences that kept the page the user chose. */
  keptManual: number;
  notice: TopicNotice | null;
}

export const topicCommands = {
  getTodayTopic: () => invoke<TodayTopic>('get_today_topic'),
  assignNextTopics: () => invoke<AssignReport>('assign_next_topics'),
  /** Refused by the core while the lock is held. */
  setTopicAssignment: (planId: number, date: string, pageId: number) =>
    invoke<void>('set_topic_assignment', { planId, date, pageId }),
  listTopicAssignments: (from: string, to: string) => invoke<TopicAssignment[]>('list_topic_assignments', { from, to }),
  getTopicHistory: (planId: number, date: string) => invoke<TopicAssignment[]>('get_topic_history', { planId, date }),
};
