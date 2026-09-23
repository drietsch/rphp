<?php
// DeflateContext / InflateContext: final, made only by *_init(), never
// cloned or serialized, and type-checked where they are taken.
$d = deflate_init(ZLIB_ENCODING_DEFLATE);
$i = inflate_init(ZLIB_ENCODING_DEFLATE);
var_dump($d, $i);
try { new DeflateContext; } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { new InflateContext; } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { serialize($d); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { serialize([$i]); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { unserialize('O:14:"InflateContext":0:{}'); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { unserialize('O:14:"DeflateContext":0:{}'); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { clone $d; } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { clone $i; } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
$r = new ReflectionClass('DeflateContext');
var_dump($r->isFinal(), $r->isInternal(), get_class_methods('DeflateContext'), get_object_vars($d));
var_dump($d instanceof DeflateContext, $i instanceof InflateContext, is_object($d));
try { deflate_add([], "y"); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { deflate_add("x", "y"); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { deflate_add($i, "y"); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { inflate_add(1, "y"); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { inflate_add($d, "y"); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { inflate_get_status($d); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { inflate_get_read_len(null); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { deflate_add($d, []); } catch (\Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
// Contexts are independent of each other.
$a = deflate_init(ZLIB_ENCODING_RAW);
$b = deflate_init(ZLIB_ENCODING_RAW, ['level' => 1]);
$x = deflate_add($a, "aaaa", ZLIB_NO_FLUSH) . deflate_add($b, "bbbb", ZLIB_NO_FLUSH);
var_dump(bin2hex(deflate_add($a, "", ZLIB_FINISH)), bin2hex(deflate_add($b, "", ZLIB_FINISH)), $x);
