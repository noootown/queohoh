const UNIT_MS: Record<string, number> = {
	s: 1_000,
	m: 60_000,
	h: 3_600_000,
	d: 86_400_000,
};

export function parseDuration(text: string): number {
	const match = /^(\d+)([smhd])$/.exec(text);
	if (!match) throw new Error(`invalid duration: ${text}`);
	const amount = Number(match[1]);
	const unit = UNIT_MS[match[2] as string];
	if (unit === undefined) throw new Error(`invalid duration: ${text}`);
	return amount * unit;
}
