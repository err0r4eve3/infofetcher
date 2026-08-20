CREATE TABLE admin_credentials (
  id BOOLEAN PRIMARY KEY DEFAULT true CHECK (id),
  username TEXT NOT NULL,
  password_hash TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
