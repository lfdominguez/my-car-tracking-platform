-- Chat with my car data: conversations + transcripts.
--
-- One table holds both the OpenRouter wire transcript and the display projection:
--   * replay layer  -> every row in `seq` order, rebuilt into OpenAI messages
--   * display layer -> role IN ('user','assistant') with non-empty content
-- Two tables would have meant keeping two transcripts in sync for no gain.

CREATE TABLE IF NOT EXISTS chat_conversations (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Optional focus for the thread. SET NULL (not CASCADE): deleting a car should
    -- not silently delete the conversation about it.
    car_id     UUID REFERENCES cars(id) ON DELETE SET NULL,
    title      TEXT NOT NULL DEFAULT 'New chat',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS chat_conversations_user_updated_idx
    ON chat_conversations (user_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS chat_messages (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    conversation_id UUID NOT NULL REFERENCES chat_conversations(id) ON DELETE CASCADE,
    seq             BIGINT NOT NULL,
    role            TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'tool')),
    content         TEXT NOT NULL DEFAULT '',
    -- Assistant turns that called tools: the OpenAI `tool_calls` array verbatim.
    tool_calls      JSONB,
    -- Tool replies: which call they answer.
    tool_call_id    TEXT,
    tool_name       TEXT,
    status          TEXT NOT NULL DEFAULT 'complete'
                    CHECK (status IN ('pending', 'running', 'complete', 'failed')),
    -- Operator-facing failure detail; never returned to the SPA verbatim.
    error           TEXT,
    -- Display-only record of which tools ran: [{name, ok, ms}].
    tool_trace      JSONB,
    model           TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (conversation_id, seq)
);

CREATE INDEX IF NOT EXISTS chat_messages_conversation_seq_idx
    ON chat_messages (conversation_id, seq);

-- Restart recovery: an in-flight generation cannot survive a process restart, and a
-- row stuck in 'running' would block the conversation's concurrency guard forever.
-- The server also runs this on boot (see chat::fail_interrupted_messages).
UPDATE chat_messages
SET status = 'failed',
    error  = 'interrupted by server restart'
WHERE status IN ('pending', 'running');
