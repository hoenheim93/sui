// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

import { Tab as HeadlessTab } from '@headlessui/react';
import clsx from 'clsx';

import { type ExtractProps } from './types';

export const TabPanels = HeadlessTab.Panels;

export type TabPanelProps = ExtractProps<typeof HeadlessTab.Panel> & {
    noGap?: boolean;
};

export function TabPanel({ noGap = false, ...props }: TabPanelProps) {
    return <HeadlessTab.Panel className={noGap ? '' : 'my-4'} {...props} />;
}

export type TabGroupProps = ExtractProps<typeof HeadlessTab.Group>;

export function TabGroup(props: TabGroupProps) {
    return <HeadlessTab.Group as="div" {...props} />;
}

export type TabProps = ExtractProps<typeof HeadlessTab>;

export function Tab({ ...props }: TabProps) {
    return (
        <HeadlessTab
            className="-mb-px border-b border-transparent pb-2 text-body font-semibold text-steel-dark hover:text-steel-darker active:text-steel ui-selected:border-gray-65 ui-selected:text-steel-darker lg:text-heading4"
            {...props}
        />
    );
}

export type TabListProps = ExtractProps<typeof HeadlessTab.List> & {
    fullWidth?: boolean;
    disableBottomBorder?: boolean;
};

export function TabList({
    fullWidth,
    disableBottomBorder,
    ...props
}: TabListProps) {
    return (
        <HeadlessTab.List
            className={clsx(
                'flex gap-6 border-gray-45',
                fullWidth && 'flex-1',
                !disableBottomBorder && 'border-b'
            )}
            {...props}
        />
    );
}
