import Foundation

/// One line from `~/.guardian/agent_queue.jsonl`.
struct AgentQueueEntry: Codable, Identifiable {
    let id: String
    let enqueuedAt: String
    let title: String
    let body: String
    let source: String
    let conversationId: String?
    /// First `workspace_roots` entry from Cursor when enqueued from a blocked submit.
    let workspacePath: String?

    enum CodingKeys: String, CodingKey {
        case id
        case enqueuedAt = "enqueued_at"
        case title, body, source
        case conversationId = "conversation_id"
        case workspacePath = "workspace_path"
    }
}
