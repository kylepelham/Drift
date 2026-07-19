import { persisted } from "./persist"

export const [selectedSession, selectSession] = persisted<string | null>("drift.session", null)
