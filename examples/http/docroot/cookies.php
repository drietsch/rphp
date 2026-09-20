<?php
setcookie("a", "b c,d;e"); setcookie("d", "e", 1800000000, "/p", "example.com", true, true);
setcookie("f", "g", ["expires" => 0, "path" => "/", "domain" => "", "secure" => false, "httponly" => false, "samesite" => "Lax"]);
setcookie("h", "", 0); setcookie("i", "x", 1); setrawcookie("j", "k%20l%20m");
setcookie("m", "v", ["samesite" => "None", "secure" => true, "partitioned" => true]);
setcookie("n", "v", ["expires" => 1700000000]);
try { var_dump(setcookie("p", "v", ["samesite" => "bogus"])); } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
setcookie("s", "v", ["expires" => -1]);
setcookie("t", "ü+&=");
setcookie("u", "v", ["expires" => 0.5, "domain" => "ex.com"]);
var_dump(setcookie("w", "v", ["expires" => "1700000000"]));
setcookie("y", "", 1800000000);
setcookie("z", "v", 1800000000, "", "", false, false);
setcookie("aa", "v", 0, "/", "", false, false);
setcookie("ab", "v", ["samesite" => "strict", "httponly" => 1]);
setcookie("ac", "v", ["expires" => 2]);
setcookie("a", "again");
header("X-Dup: 1"); header("X-Dup: 2", false);
var_dump(headers_list());
