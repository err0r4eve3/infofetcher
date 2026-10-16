-- Evidence is retained independently of live reader content.
CREATE TABLE osint_documents (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    case_id uuid NOT NULL REFERENCES osint_cases(id) ON DELETE RESTRICT,
    filename text NOT NULL,
    media_type text NOT NULL CHECK (media_type IN ('application/pdf', 'image/png', 'image/jpeg', 'text/plain')),
    body bytea NOT NULL CHECK (octet_length(body) BETWEEN 1 AND 10485760),
    sha256 text NOT NULL CHECK (sha256 ~ '^[a-f0-9]{64}$'),
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by text NOT NULL,
    status text NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'ready', 'failed', 'cancelled')),
    extraction jsonb,
    extraction_sha256 text CHECK (extraction_sha256 ~ '^[a-f0-9]{64}$'),
    UNIQUE (case_id, sha256)
);
CREATE INDEX osint_documents_case ON osint_documents(case_id, created_at DESC);

CREATE TABLE osint_jobs (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    case_id uuid NOT NULL REFERENCES osint_cases(id) ON DELETE RESTRICT,
    document_id uuid REFERENCES osint_documents(id) ON DELETE RESTRICT,
    kind text NOT NULL CHECK (kind IN ('document', 'research')),
    input jsonb NOT NULL,
    created_by text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    status text NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'done', 'failed', 'cancelled')),
    attempts integer NOT NULL DEFAULT 0,
    lease_token uuid,
    lease_until timestamptz,
    error text,
    run_id uuid REFERENCES osint_runs(id) ON DELETE RESTRICT,
    CHECK ((kind = 'document') = (document_id IS NOT NULL))
);
CREATE UNIQUE INDEX osint_jobs_document ON osint_jobs(document_id) WHERE document_id IS NOT NULL;
CREATE UNIQUE INDEX osint_jobs_active_research ON osint_jobs(case_id) WHERE kind = 'research' AND status IN ('queued', 'running');
CREATE INDEX osint_jobs_pending ON osint_jobs(status, created_at);
CREATE INDEX osint_jobs_case ON osint_jobs(case_id, created_at DESC);
