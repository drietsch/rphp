<?php
// json_encode of enum cases: a backed case is its value, a pure case has no
// serialization (JSON_ERROR_NON_BACKED_ENUM; `0` under partial output).
enum P { case A; case B; }
enum Q: string { case X = 'x'; }
enum R: int { case I = 3; }
var_dump(json_encode(P::A), json_last_error(), json_last_error_msg());
var_dump(json_encode(['t' => [P::A, Q::X, R::I]]), json_last_error_msg());
var_dump(json_encode([Q::X, R::I], JSON_PRETTY_PRINT));
try { json_encode(P::B, JSON_THROW_ON_ERROR); } catch (JsonException $e) { echo get_class($e), ': ', $e->getMessage(), ' ', $e->getCode(), "\n"; }
var_dump(json_encode(P::A, JSON_PARTIAL_OUTPUT_ON_ERROR), json_last_error_msg());
var_dump(json_encode(['a' => P::A, 'b' => 1], JSON_PARTIAL_OUTPUT_ON_ERROR));
class J implements JsonSerializable { function jsonSerialize(): mixed { return P::A; } }
var_dump(json_encode(new J), json_last_error_msg());
