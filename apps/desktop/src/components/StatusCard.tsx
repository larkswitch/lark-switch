import type { ReactNode } from 'react';
import { Card, Typography } from '@douyinfe/semi-ui';
import { IconTickCircle } from '@douyinfe/semi-icons';

const { Title, Text } = Typography;

export interface StatusCardProps {
  icon: ReactNode;
  title: string;
  value: string;
  detail: string;
  ok: boolean;
  extra?: ReactNode;
}

export function StatusCard(props: StatusCardProps) {
  return (
    <Card className="status-card">
      <div className="status-icon">{props.icon}</div>
      <Text type="tertiary">{props.title}</Text>
      <Title heading={4}>{props.value}</Title>
      <Text type="tertiary" ellipsis={{ showTooltip: true }}>{props.detail}</Text>
      {props.ok && <IconTickCircle className="status-ok" />}
      {props.extra ? <div className="card-actions">{props.extra}</div> : null}
    </Card>
  );
}
