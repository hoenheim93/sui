// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

import { RadioGroup as HeadlessRadioGroup } from '@headlessui/react';
import { type ReactNode } from 'react';

import { type ExtractProps } from '~/ui/types';

export type RadioGroupProps = ExtractProps<typeof HeadlessRadioGroup> & {
    children: ReactNode;
    ariaLabel: string;
};

export function RadioGroup({ ariaLabel, children, ...props }: RadioGroupProps) {
    return (
        <HeadlessRadioGroup {...props}>
            <HeadlessRadioGroup.Label className="sr-only">
                {ariaLabel}
            </HeadlessRadioGroup.Label>
            {children}
        </HeadlessRadioGroup>
    );
}

export type RadioOptionProps = ExtractProps<
    typeof HeadlessRadioGroup.Option
> & {
    title?: string;
    description?: string;
    children?: ReactNode;
};

export function RadioOption({
    title,
    description,
    children,
    ...props
}: RadioOptionProps) {
    return (
        <HeadlessRadioGroup.Option
            className="cursor-pointer rounded-md border border-transparent bg-white text-steel-dark hover:text-steel-darker active:text-steel ui-checked:border-steel"
            {...props}
        >
            {title && (
                <HeadlessRadioGroup.Label className="text-caption-small px-2 py-1 font-semibold">
                    {title}
                </HeadlessRadioGroup.Label>
            )}
            {description && (
                <HeadlessRadioGroup.Description className="px-2 py-1 text-bodySmall">
                    {description}
                </HeadlessRadioGroup.Description>
            )}
            {children}
        </HeadlessRadioGroup.Option>
    );
}
