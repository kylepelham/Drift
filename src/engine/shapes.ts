// The message, part, session and event shapes the UI renders; src/engine/native/adapt.ts builds them from the
// native engine's. Taken unchanged from @opencode-ai/sdk 1.18.33, whose reducer and views the UI kept.

export type EventServerInstanceDisposed = {
  type: "server.instance.disposed"
  properties: {
    directory: string
  }
}

export type EventInstallationUpdated = {
  type: "installation.updated"
  properties: {
    version: string
  }
}

export type EventInstallationUpdateAvailable = {
  type: "installation.update-available"
  properties: {
    version: string
  }
}

export type EventLspClientDiagnostics = {
  type: "lsp.client.diagnostics"
  properties: {
    serverID: string
    path: string
  }
}

export type EventLspUpdated = {
  type: "lsp.updated"
  properties: {
    [key: string]: unknown
  }
}

export type FileDiff = {
  file: string
  before: string
  after: string
  additions: number
  deletions: number
}

export type UserMessage = {
  id: string
  sessionID: string
  role: "user"
  time: {
    created: number
  }
  summary?: {
    title?: string
    body?: string
    diffs: Array<FileDiff>
  }
  agent: string
  model: {
    providerID: string
    modelID: string
  }
  system?: string
  tools?: {
    [key: string]: boolean
  }
}

export type ProviderAuthError = {
  name: "ProviderAuthError"
  data: {
    providerID: string
    message: string
  }
}

export type UnknownError = {
  name: "UnknownError"
  data: {
    message: string
  }
}

export type MessageOutputLengthError = {
  name: "MessageOutputLengthError"
  data: {
    [key: string]: unknown
  }
}

export type MessageAbortedError = {
  name: "MessageAbortedError"
  data: {
    message: string
  }
}

export type ApiError = {
  name: "APIError"
  data: {
    message: string
    statusCode?: number
    isRetryable: boolean
    responseHeaders?: {
      [key: string]: string
    }
    responseBody?: string
  }
}

export type AssistantMessage = {
  id: string
  sessionID: string
  role: "assistant"
  time: {
    created: number
    completed?: number
  }
  error?: ProviderAuthError | UnknownError | MessageOutputLengthError | MessageAbortedError | ApiError
  parentID: string
  modelID: string
  providerID: string
  mode: string
  path: {
    cwd: string
    root: string
  }
  summary?: boolean
  cost: number
  tokens: {
    input: number
    output: number
    reasoning: number
    cache: {
      read: number
      write: number
    }
  }
  finish?: string
}

export type Message = UserMessage | AssistantMessage

export type EventMessageUpdated = {
  type: "message.updated"
  properties: {
    info: Message
  }
}

export type EventMessageRemoved = {
  type: "message.removed"
  properties: {
    sessionID: string
    messageID: string
  }
}

export type TextPart = {
  id: string
  sessionID: string
  messageID: string
  type: "text"
  text: string
  synthetic?: boolean
  ignored?: boolean
  time?: {
    start: number
    end?: number
  }
  metadata?: {
    [key: string]: unknown
  }
}

export type ReasoningPart = {
  id: string
  sessionID: string
  messageID: string
  type: "reasoning"
  text: string
  metadata?: {
    [key: string]: unknown
  }
  time: {
    start: number
    end?: number
  }
}

export type FilePartSourceText = {
  value: string
  start: number
  end: number
}

export type FileSource = {
  text: FilePartSourceText
  type: "file"
  path: string
}

export type Range = {
  start: {
    line: number
    character: number
  }
  end: {
    line: number
    character: number
  }
}

export type SymbolSource = {
  text: FilePartSourceText
  type: "symbol"
  path: string
  range: Range
  name: string
  kind: number
}

export type FilePartSource = FileSource | SymbolSource

export type FilePart = {
  id: string
  sessionID: string
  messageID: string
  type: "file"
  mime: string
  filename?: string
  url: string
  source?: FilePartSource
}

export type ToolStatePending = {
  status: "pending"
  input: {
    [key: string]: unknown
  }
  raw: string
}

export type ToolStateRunning = {
  status: "running"
  input: {
    [key: string]: unknown
  }
  title?: string
  metadata?: {
    [key: string]: unknown
  }
  time: {
    start: number
  }
}

export type ToolStateCompleted = {
  status: "completed"
  input: {
    [key: string]: unknown
  }
  output: string
  title: string
  metadata: {
    [key: string]: unknown
  }
  time: {
    start: number
    end: number
    compacted?: number
  }
  attachments?: Array<FilePart>
}

export type ToolStateError = {
  status: "error"
  input: {
    [key: string]: unknown
  }
  error: string
  metadata?: {
    [key: string]: unknown
  }
  time: {
    start: number
    end: number
  }
}

export type ToolState = ToolStatePending | ToolStateRunning | ToolStateCompleted | ToolStateError

export type ToolPart = {
  id: string
  sessionID: string
  messageID: string
  type: "tool"
  callID: string
  tool: string
  state: ToolState
  metadata?: {
    [key: string]: unknown
  }
}

export type StepStartPart = {
  id: string
  sessionID: string
  messageID: string
  type: "step-start"
  snapshot?: string
}

export type StepFinishPart = {
  id: string
  sessionID: string
  messageID: string
  type: "step-finish"
  reason: string
  snapshot?: string
  cost: number
  tokens: {
    input: number
    output: number
    reasoning: number
    cache: {
      read: number
      write: number
    }
  }
}

export type SnapshotPart = {
  id: string
  sessionID: string
  messageID: string
  type: "snapshot"
  snapshot: string
}

export type PatchPart = {
  id: string
  sessionID: string
  messageID: string
  type: "patch"
  hash: string
  files: Array<string>
}

export type AgentPart = {
  id: string
  sessionID: string
  messageID: string
  type: "agent"
  name: string
  source?: {
    value: string
    start: number
    end: number
  }
}

export type RetryPart = {
  id: string
  sessionID: string
  messageID: string
  type: "retry"
  attempt: number
  error: ApiError
  time: {
    created: number
  }
}

export type CompactionPart = {
  id: string
  sessionID: string
  messageID: string
  type: "compaction"
  auto: boolean
}

/** A plugin's words: context beside a prompt, a prompt of its own, or a note under a reply. */
export type PluginPart = {
  id: string
  sessionID: string
  messageID: string
  type: "plugin"
  plugin: string
  text: string
}

export type Part =
  | TextPart
  | {
      id: string
      sessionID: string
      messageID: string
      type: "subtask"
      prompt: string
      description: string
      agent: string
    }
  | ReasoningPart
  | FilePart
  | ToolPart
  | StepStartPart
  | StepFinishPart
  | SnapshotPart
  | PatchPart
  | AgentPart
  | RetryPart
  | CompactionPart
  | PluginPart

export type EventMessagePartUpdated = {
  type: "message.part.updated"
  properties: {
    part: Part
    delta?: string
  }
}

export type EventMessagePartRemoved = {
  type: "message.part.removed"
  properties: {
    sessionID: string
    messageID: string
    partID: string
  }
}

export type Permission = {
  id: string
  type: string
  pattern?: string | Array<string>
  sessionID: string
  messageID: string
  callID?: string
  title: string
  metadata: {
    [key: string]: unknown
  }
  time: {
    created: number
  }
}

export type EventPermissionUpdated = {
  type: "permission.updated"
  properties: Permission
}

export type EventPermissionReplied = {
  type: "permission.replied"
  properties: {
    sessionID: string
    permissionID: string
    response: string
  }
}

export type SessionStatus =
  | {
      type: "idle"
    }
  | {
      type: "retry"
      attempt: number
      message: string
      next: number
    }
  | {
      type: "busy"
    }

export type EventSessionStatus = {
  type: "session.status"
  properties: {
    sessionID: string
    status: SessionStatus
  }
}

export type EventSessionIdle = {
  type: "session.idle"
  properties: {
    sessionID: string
  }
}

export type EventSessionCompacted = {
  type: "session.compacted"
  properties: {
    sessionID: string
  }
}

export type EventFileEdited = {
  type: "file.edited"
  properties: {
    file: string
  }
}

export type Todo = {
  /**
   * Brief description of the task
   */
  content: string
  /**
   * Current status of the task: pending, in_progress, completed, cancelled
   */
  status: string
  /**
   * Priority level of the task: high, medium, low
   */
  priority: string
  /**
   * Unique identifier for the todo item
   */
  id: string
}

export type EventTodoUpdated = {
  type: "todo.updated"
  properties: {
    sessionID: string
    todos: Array<Todo>
  }
}

export type EventCommandExecuted = {
  type: "command.executed"
  properties: {
    name: string
    sessionID: string
    arguments: string
    messageID: string
  }
}

export type Session = {
  id: string
  projectID: string
  directory: string
  parentID?: string
  /** Answers its own asks, and its subagents', except secrets and anything outside the workspace. */
  autoAccept?: boolean
  summary?: {
    additions: number
    deletions: number
    files: number
    diffs?: Array<FileDiff>
  }
  share?: {
    url: string
  }
  title: string
  version: string
  time: {
    created: number
    updated: number
    compacting?: number
  }
  revert?: {
    messageID: string
    partID?: string
    snapshot?: string
    diff?: string
  }
}

export type EventSessionCreated = {
  type: "session.created"
  properties: {
    info: Session
  }
}

export type EventSessionUpdated = {
  type: "session.updated"
  properties: {
    info: Session
  }
}

export type EventSessionDeleted = {
  type: "session.deleted"
  properties: {
    info: Session
  }
}

export type EventSessionDiff = {
  type: "session.diff"
  properties: {
    sessionID: string
    diff: Array<FileDiff>
  }
}

export type EventSessionError = {
  type: "session.error"
  properties: {
    sessionID?: string
    error?: ProviderAuthError | UnknownError | MessageOutputLengthError | MessageAbortedError | ApiError
  }
}

export type EventFileWatcherUpdated = {
  type: "file.watcher.updated"
  properties: {
    file: string
    event: "add" | "change" | "unlink"
  }
}

export type EventVcsBranchUpdated = {
  type: "vcs.branch.updated"
  properties: {
    branch?: string
  }
}

export type EventTuiPromptAppend = {
  type: "tui.prompt.append"
  properties: {
    text: string
  }
}

export type EventTuiCommandExecute = {
  type: "tui.command.execute"
  properties: {
    command:
      | (
          | "session.list"
          | "session.new"
          | "session.share"
          | "session.interrupt"
          | "session.compact"
          | "session.page.up"
          | "session.page.down"
          | "session.half.page.up"
          | "session.half.page.down"
          | "session.first"
          | "session.last"
          | "prompt.clear"
          | "prompt.submit"
          | "agent.cycle"
        )
      | string
  }
}

export type EventTuiToastShow = {
  type: "tui.toast.show"
  properties: {
    title?: string
    message: string
    variant: "info" | "success" | "warning" | "error"
    /**
     * Duration in milliseconds
     */
    duration?: number
  }
}

export type Pty = {
  id: string
  title: string
  command: string
  args: Array<string>
  cwd: string
  status: "running" | "exited"
  pid: number
}

export type EventPtyCreated = {
  type: "pty.created"
  properties: {
    info: Pty
  }
}

export type EventPtyUpdated = {
  type: "pty.updated"
  properties: {
    info: Pty
  }
}

export type EventPtyExited = {
  type: "pty.exited"
  properties: {
    id: string
    exitCode: number
  }
}

export type EventPtyDeleted = {
  type: "pty.deleted"
  properties: {
    id: string
  }
}

export type EventServerConnected = {
  type: "server.connected"
  properties: {
    [key: string]: unknown
  }
}

export type Event =
  | EventServerInstanceDisposed
  | EventInstallationUpdated
  | EventInstallationUpdateAvailable
  | EventLspClientDiagnostics
  | EventLspUpdated
  | EventMessageUpdated
  | EventMessageRemoved
  | EventMessagePartUpdated
  | EventMessagePartRemoved
  | EventPermissionUpdated
  | EventPermissionReplied
  | EventSessionStatus
  | EventSessionIdle
  | EventSessionCompacted
  | EventFileEdited
  | EventTodoUpdated
  | EventCommandExecuted
  | EventSessionCreated
  | EventSessionUpdated
  | EventSessionDeleted
  | EventSessionDiff
  | EventSessionError
  | EventFileWatcherUpdated
  | EventVcsBranchUpdated
  | EventTuiPromptAppend
  | EventTuiCommandExecute
  | EventTuiToastShow
  | EventPtyCreated
  | EventPtyUpdated
  | EventPtyExited
  | EventPtyDeleted
  | EventServerConnected

export type Command = {
  name: string
  description?: string
  agent?: string
  model?: string
  template: string
  subtask?: boolean
}

export type Model = {
  id: string
  providerID: string
  api: {
    id: string
    url: string
    npm: string
  }
  name: string
  capabilities: {
    temperature: boolean
    reasoning: boolean
    attachment: boolean
    toolcall: boolean
    input: {
      text: boolean
      audio: boolean
      image: boolean
      video: boolean
      pdf: boolean
    }
    output: {
      text: boolean
      audio: boolean
      image: boolean
      video: boolean
      pdf: boolean
    }
  }
  cost: {
    input: number
    output: number
    cache: {
      read: number
      write: number
    }
    experimentalOver200K?: {
      input: number
      output: number
      cache: {
        read: number
        write: number
      }
    }
  }
  limit: {
    context: number
    output: number
  }
  status: "alpha" | "beta" | "deprecated" | "active"
  options: {
    [key: string]: unknown
  }
  headers: {
    [key: string]: string
  }
}
