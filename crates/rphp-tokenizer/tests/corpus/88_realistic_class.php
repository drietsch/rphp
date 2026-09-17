<?php

declare(strict_types=1);

namespace App\Http\Controller;

use App\Entity\{User, Post as BlogPost};
use function array_map;
use const PHP_EOL;

/**
 * A realistic file: attributes, enums, hooks, readonly, match, closures.
 */
#[AsController]
final class UserController extends AbstractController implements \Countable
{
    public const string NAME = 'user';
    private static ?self $instance = null;

    public function __construct(
        private readonly UserRepository $users,
        public protected(set) int $count = 0,
        #[Autowire('%kernel.debug%')] bool $debug = false,
    ) {}

    public string $title {
        get => $this->title ?? 'untitled';
        set(string $value) { $this->title = trim($value); }
    }

    #[Route('/users/{id}', methods: ['GET'])]
    public function show(int $id, ?Request $request = null): Response
    {
        $user = $this->users->find($id) ?? throw new NotFoundException("User #{$id} not found");
        $label = match (true) {
            $user->isAdmin() => 'admin',
            $user instanceof BlogPost, $id > 10 => "post-{$user->slug}",
            default => sprintf('%s (%d)', $user->name, $id),
        };
        $fn = static fn(array $xs): array => array_map(fn($x) => $x?->id ?? -1, $xs);
        $lines = <<<TXT
            Hello {$user->name},
              your id is $id and your label is {$label}.
            Bye \$user
            TXT;
        $sql = <<<'SQL'
            SELECT * FROM users WHERE id = :id
            SQL;
        yield from $this->stream($lines . PHP_EOL, $sql);
        return new Response($lines, (int) $request?->query->get('status', 200), [...$this->headers(), 'X-Debug' => (string) $this->debug]);
    }

    public function count(): int { return $this->count; }

    private function &ref(): array { static $cache = []; return $cache; }
}

enum Status: string
{
    case Active = 'active';
    case Blocked = 'blocked';

    public function label(): string
    {
        return ucfirst($this->value) . " ({$this->name})";
    }
}

function helper(int|string ...$args): never { exit(1); }
?>
<div><?= $x ?></div>
<?php if ($a): ?>
  yes
<?php elseif ($b): ?>
  maybe
<?php else: ?>
  no
<?php endif; ?>
