/**
 * Subagentシステムの型定義
 */

export type AgentStatus = 'idle' | 'running' | 'waiting' | 'completed' | 'failed';

export type MessageType = 'task_request' | 'task_result' | 'progress' | 'error' | 'heartbeat';

export type TaskStatus = 'pending' | 'running' | 'completed' | 'failed' | 'cancelled';

export type ExecutionPattern = 'sequential' | 'parallel' | 'hierarchical' | 'conditional';

export type Priority = 'low' | 'medium' | 'high' | 'critical';

export interface Task {
  id: string;
  action: string;
  parameters: Record<string, any>;
  context?: TaskContext;
  metadata?: TaskMetadata;
}

export interface TaskContext {
  parent_task?: string;
  deadline?: string;
  shared_context?: SharedContext;
  [key: string]: any;
}

export interface TaskMetadata {
  priority?: Priority;
  correlation_id?: string;
  retry_count?: number;
  created_at?: string;
  [key: string]: any;
}

export interface TaskResult {
  taskId: string;
  status: TaskStatus;
  result?: any;
  error?: ErrorInfo;
  confidence?: number;
  metrics?: TaskMetrics;
}

export interface ErrorInfo {
  code: string;
  message: string;
  details?: any;
  stack?: string;
}

export interface TaskMetrics {
  duration_ms: number;
  tokens_used?: number;
  tools_called?: number;
  [key: string]: any;
}

export interface SharedContext {
  session_id?: string;
  user_id?: string;
  workspace?: string;
  [key: string]: any;
}

export interface SubagentState {
  id: string;
  status: AgentStatus;
  currentTask: Task | null;
  context: {
    memory: Map<string, any>;
    history: TaskResult[];
    sharedContext?: SharedContext;
  };
  metrics: {
    tasksCompleted: number;
    avgDuration: number;
    successRate: number;
  };
}

export interface A2AMessage {
  message_id: string;
  timestamp: string;
  from: string;
  to: string;
  type: MessageType;
  payload?: any;
  metadata?: MessageMetadata;
}

export interface MessageMetadata {
  priority?: Priority;
  correlation_id?: string;
  [key: string]: any;
}

export interface TaskRequestPayload {
  task_id: string;
  action: string;
  parameters: Record<string, any>;
  context?: TaskContext;
}

export interface TaskResultPayload {
  task_id: string;
  result?: any;
  error?: ErrorInfo;
}

export interface A2AResponse extends A2AMessage {
  in_reply_to: string;
  status?: TaskStatus;
  metrics?: TaskMetrics;
}

export interface Tool {
  name: string;
  description: string;
  parameters: Record<string, any>;
  invoke(params: Record<string, any>): Promise<any>;
}

export interface SubagentConfig {
  id: string;
  name: string;
  version: string;
  role: RoleConfig;
  interface: InterfaceConfig;
  tools: string[];
  behavior: BehaviorConfig;
}

export interface RoleConfig {
  description: string;
  capabilities: string[];
  constraints?: Record<string, any>;
}

export interface InterfaceConfig {
  input: ParameterDefinition[];
  output: ParameterDefinition[];
}

export interface ParameterDefinition {
  name: string;
  type: string;
  required: boolean;
  schema?: any;
}

export interface BehaviorConfig {
  retry_policy?: RetryPolicy;
  timeout?: number;
  fallback?: string;
}

export interface RetryPolicy {
  max_attempts: number;
  backoff: 'exponential' | 'linear' | 'fixed';
  initial_delay_ms?: number;
}

export interface ErrorHandlingStrategy {
  timeout?: ErrorAction;
  validation_error?: ErrorAction;
  tool_failure?: ErrorAction;
  partial_failure?: ErrorAction;
}

export interface ErrorAction {
  action: string;
  max_retries?: number;
  fallback?: string;
  alternatives?: ToolAlternative[];
  escalate_after?: number;
  notify?: boolean;
}

export interface ToolAlternative {
  primary: string;
  backup: string;
}

export interface OrchestrationPlan {
  pattern: ExecutionPattern;
  tasks: Task[];
  dependencies?: Map<string, string[]>;
  conditions?: ConditionalRule[];
}

export interface ConditionalRule {
  condition: string;
  target_subagent: string;
  action: string;
}

export interface ProgressReport {
  taskId: string;
  progress: number;
  message: string;
  timestamp: string;
}

