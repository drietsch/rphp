<?php
// UConverter subclasses: toUCallback/fromUCallback see every fault with
// ICU's reason, source, code units and error code (twice: ICU's preflight
// pass and the real one, each after a REASON_RESET), and what they return
// is written in its place.
class Logging extends UConverter {
    public function toUCallback($reason, $source, $codeUnits, &$error): string|int|array|null {
        // (REASON_CLOSE, when the object is freed, is not logged: rphp
        // does not call back on close.)
        if ($reason === UConverter::REASON_CLOSE) return null;
        echo 'toU(', UConverter::reasonText($reason), ', ', bin2hex($source), ', ', bin2hex($codeUnits), ", $error)\n";
        if ($reason <= 2) { $error = U_ZERO_ERROR; return '?'; }
        return null;
    }
    public function fromUCallback($reason, $source, $codePoint, &$error): string|int|array|null {
        if ($reason === UConverter::REASON_CLOSE) return null;
        echo 'fromU(', UConverter::reasonText($reason), ', ', json_encode($source), ", $codePoint, $error)\n";
        if ($reason <= 2) { $error = U_ZERO_ERROR; return '[' . dechex($codePoint) . ']'; }
        return null;
    }
}
$c = new Logging('ISO-8859-1', 'UTF-8');
var_dump($c->convert("a\xffb€c😀d\xe2\x82"));
var_dump(bin2hex($c->convert("a\xe9b", true)));
$d = new Logging('US-ASCII', 'ISO-8859-1');
var_dump($d->convert("a\xe9\x80b"));
$e = new Logging('UTF-8', 'UTF-16LE');
var_dump($e->convert("a\x00\x00\xd8b\x00"));
var_dump($e->convert("a\x00b"));
$k = new Logging('KOI8-R', 'UTF-8');
var_dump(bin2hex($k->convert("Жa\u{200B}é")));
class Parent_ extends UConverter {
    public function fromUCallback($reason, $source, $codePoint, &$error): string|int|array|null {
        return parent::fromUCallback($reason, $source, $codePoint, $error);
    }
}
$f = new Parent_('US-ASCII', 'UTF-8');
var_dump(bin2hex($f->convert('aéb')), $f->getErrorCode());
class Keeps extends UConverter {
    public function fromUCallback($reason, $source, $codePoint, &$error): string|int|array|null { return $reason <= 2 ? 65 : null; }
}
$g = new Keeps('US-ASCII', 'UTF-8');
var_dump($g->convert('aéb'), $g->getErrorCode(), $g->getErrorMessage());
class Arrays extends UConverter {
    public function toUCallback($reason, $source, $codeUnits, &$error): string|int|array|null {
        if ($reason > 2) return null;
        $error = 0;
        return [0x263A, 'x', [0x1F600]];
    }
    public function fromUCallback($reason, $source, $codePoint, &$error): string|int|array|null {
        if ($reason > 2) return null;
        $error = 0;
        return [0x2A, '<>', null];
    }
}
$h = new Arrays('US-ASCII', 'UTF-8');
var_dump($h->convert("a\xffb"), $h->convert("é"));
