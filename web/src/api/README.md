# API contract boundary

Rust owns the domain and API schemas. Generated TypeScript types and clients will
be added with T012/T074; do not hand-maintain duplicate backend shapes here.
The T006 preview makes no API requests and declares no domain or wire types.
Graph interactions and ELK layout remain T083; their dependency probe is separate.
