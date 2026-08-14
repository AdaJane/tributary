import { describe, expect, it } from 'vitest';

import { jacksFedBy, outputKey, patchAt, sameSource } from './output-patch';
import type { OutputPatch, OutputSource } from './output-patch';

const master: OutputSource = { kind: 'master' };
const strip0: OutputSource = { kind: 'strip', id: 0 };

function patch(device: string | null, channel: number, source = master): OutputPatch {
  return {
    ...(device === null ? {} : { device }),
    channel,
    source_channel: 0,
    tap: 'pre_fader',
    source,
  } as OutputPatch;
}

describe('output patch identity', () => {
  it('an output key can never collide with an input jack key', () => {
    // Both sides are (device, channel). Without the prefix a future map
    // keyed on either would confuse a patch with a jack — and the input
    // side hashes that key into the link tape's colour.
    expect(outputKey('interface', 3)).toBe('out:interface#3');
    expect(outputKey('interface', 3)).not.toBe('interface#3');
  });

  it('channel 3 on two devices is two different jacks', () => {
    expect(outputKey('a', 3)).not.toBe(outputKey('b', 3));
    expect(outputKey(null, 3)).not.toBe(outputKey('a', 3));
  });

  it('finds the patch on a jack and treats the default device as its own', () => {
    const patches = [patch('interface', 3), patch(null, 3)];
    expect(patchAt(patches, 'interface', 3)?.device).toBe('interface');
    expect(patchAt(patches, null, 3)?.device).toBeUndefined();
    expect(patchAt(patches, 'interface', 4)).toBeNull();
  });

  it('one source may feed several jacks', () => {
    // The mirror invariant of one input jack feeding several strips. Any
    // lookup keyed on the source rather than the jack loses this.
    const patches = [patch('interface', 3), patch('interface', 5), patch('interface', 6, strip0)];
    expect(jacksFedBy(patches, master)).toHaveLength(2);
    expect(jacksFedBy(patches, strip0)).toHaveLength(1);
  });

  it('two strips are different sources even at the same channel', () => {
    expect(sameSource(strip0, { kind: 'strip', id: 1 })).toBe(false);
    expect(sameSource(master, master)).toBe(true);
  });
});
