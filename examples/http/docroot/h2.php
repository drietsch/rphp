<?php header('X-A: 1'); header('x-b: 2'); header('X-A: 3', false); echo "<x>\n"; trigger_error("warn <b>", E_USER_WARNING); undefined_fn();
