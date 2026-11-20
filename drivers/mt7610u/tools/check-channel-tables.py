#!/usr/bin/env python3
"""Compare checked-in RF tables with the pinned Linux mt76 source checkout."""
import argparse
from pathlib import Path
import re
import subprocess

REVISION = '22f7ff9be59890cb10cde16cc25a30f78b504f23'
FIELDS = ['r37', 'r36', 'r35', 'r34', 'r33', 'r32_hi', 'r32_lo',
          'r31_hi', 'r31_lo', 'r30_reset', 'r30_mash', 'r30_bp', 'pll_n',
          'r28_iso', 'r28_delay', 'r28_clock', 'sdm_k', 'xo_div']
BANDS = dict(zip(['RF_G_BAND', 'RF_A_BAND', 'RF_A_BAND_LB', 'RF_A_BAND_MB',
                 'RF_A_BAND_HB', 'RF_A_BAND_11J'],
                [0x100, 0x200, 0x400, 0x800, 0x1000, 0x2000]))


def check(source):
    # Read committed contents: local edits to the reference cannot alter the oracle.
    revision = subprocess.check_output(
        ['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
    if revision != REVISION:
        raise ValueError(f'Expected upstream revision {REVISION}, found {revision}')
    upstream = subprocess.check_output(
        ['git', '-C', str(source), 'show', f'{REVISION}:mt76x0/initvals_phy.h'],
        text=True)
    rust = (Path(__file__).resolve().parents[1] / 'src/channel_tables.rs').read_text()

    def block(name):
        return upstream.split(name + '[] = {', 1)[1].split('};', 1)[0]

    def plans(name):
        return {int(values[0]): values
                for row in re.findall(r'\{([^{}]+)\}', block(name))
                if (values := [x.strip() for x in row.split(',')])}

    normal = plans('mt76x0_frequency_plan')
    special = plans('mt76x0_sdm_frequency_plan')
    sdm = set(map(int, re.findall(r'\d+', block('mt76x0_sdm_channel'))))
    expected_channels = list(range(1, 14)) + list(range(36, 65, 4)) + list(range(100, 145, 4)) + list(range(149, 166, 4))
    actual_channels = []
    for row in re.findall(r'ChannelPlan \{ channel: ([^}]+)\}', rust):
        fields = dict(part.strip().split(': ') for part in ('channel: ' + row).split(',') if part.strip())
        channel = int(fields['channel'])
        actual_channels.append(channel)
        linux = (special if channel in sdm else normal)[channel]
        expected = {name: int(value, 0) for name, value in zip(FIELDS, linux[2:], strict=True)}
        expected['band'] = sum(BANDS[name] for name in re.findall(r'RF_\w+', normal[channel][1]))
        for name, value in expected.items():
            if int(fields[name], 0) != value:
                raise ValueError(f'Channel {channel}: {name} differs from Linux')
        if fields['sdm'] != str(channel in sdm).lower():
            raise ValueError(f'Channel {channel}: incorrect SDM selection')
    if actual_channels != expected_channels:
        raise ValueError('Missing, duplicate, or unexpected channel entries')
    expected_switch = []
    for bank, register, band, value in re.findall(
            r'\{ MT_RF\(\s*(\d+),\s*(\d+)\),\s*(RF_\w+),\s*(0x[\dA-Fa-f]+)\s*\}',
            block('mt76x0_rf_band_switch_tab')):
        expected_switch.append(((int(bank) << 16) | int(register), BANDS[band], int(value, 0)))
    actual_switch = [tuple(int(x.strip(), 0) for x in row.split(','))
                     for row in re.findall(r'\((0x[\da-f]+, 0x[\da-f]+, 0x[\da-f]+)\)',
                                           rust.split('const BAND_SWITCH:', 1)[1])]
    if actual_switch != expected_switch:
        raise ValueError('Band-switch values or write order differ from Linux')
    print(f'Validated {len(actual_channels)} channel plans and {len(actual_switch)} band-switch entries against {REVISION}')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path, help='Pinned mt76 Git checkout')
    check(parser.parse_args().source)
