-- SQL example for syntax highlighting demo.

CREATE TABLE files (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    size_bytes  INTEGER NOT NULL DEFAULT 0,
    modified_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO files (name, size_bytes) VALUES
    ('hello.rs', 378),
    ('data.json', 258);

SELECT name, size_bytes
FROM files
WHERE size_bytes > 100
ORDER BY size_bytes DESC
LIMIT 10;
