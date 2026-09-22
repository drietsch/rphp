<?php
// The host lookups beside the transports. Only what is the same on every
// machine is printed: the loopback names, and the *shape* of the rest.

var_dump(strlen(gethostname()) > 0);

var_dump(gethostbyname('localhost'));
var_dump(gethostbyaddr('127.0.0.1'));

// A name that does not resolve comes back unchanged, which is php's way of
// having no error to report.
var_dump(gethostbyname('no-such-host.invalid'));
var_dump(gethostbynamel('no-such-host.invalid'));

$l = gethostbynamel('localhost');
var_dump(is_array($l), in_array('127.0.0.1', $l, true));

// An argument that is not an address at all is the one case that warns.
var_dump(gethostbyaddr('not-an-address'));
