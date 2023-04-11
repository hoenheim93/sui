// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

import { type StoryObj, type Meta } from '@storybook/react';
import { useState } from 'react';

import { RadioGroup, type RadioGroupProps, RadioOption } from '~/ui/Radio';

export default {
    component: RadioGroup,
} as Meta;

const groups = [
    {
        label: 'label 1',
        description: 'description 1',
    },
    {
        label: 'label 2',
        description: 'description 2',
    },
    {
        label: 'label 3',
        description: 'description 3',
    },
];

export const Default: StoryObj<RadioGroupProps> = {
    render: (props) => {
        const [selected, setSelected] = useState(groups[0]);

        return (
            <div>
                <RadioGroup
                    {...props}
                    className="flex"
                    value={selected}
                    onChange={setSelected}
                >
                    {groups.map((group) => (
                        <RadioOption
                            key={group.label}
                            value={group}
                            title={group.label}
                            description={group.description}
                        />
                    ))}
                </RadioGroup>
            </div>
        );
    },
};

export const CustomContent: StoryObj<RadioGroupProps> = {
    render: (props) => {
        const [selected, setSelected] = useState(groups[0]);

        return (
            <div>
                <RadioGroup
                    {...props}
                    className="flex flex-col gap-2"
                    value={selected}
                    onChange={setSelected}
                >
                    {groups.map((group) => (
                        <RadioOption
                            key={group.label}
                            value={group}
                            title={group.label}
                            description={group.description}
                        >
                            Other Custom Content
                        </RadioOption>
                    ))}
                </RadioGroup>
            </div>
        );
    },
};
