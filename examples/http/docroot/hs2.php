<?php echo str_repeat('y', 5000); var_dump(headers_sent()); header('X-After-Big: 1'); echo "\n";
