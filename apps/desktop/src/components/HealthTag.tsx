import { Tag } from '@douyinfe/semi-ui';
import { copy } from '../copy';
import type { AccountHealth } from '../types';

export interface HealthTagProps {
  health: AccountHealth;
  running: number;
}

const healthMap: Record<AccountHealth, { label: string; color: 'green' | 'orange' | 'red' | 'grey' }> = {
  ready: { label: copy.health.ready, color: 'green' },
  refreshable: { label: copy.health.refreshable, color: 'green' },
  reauth_required: { label: copy.health.reauthRequired, color: 'red' },
  temporary_failure: { label: copy.health.temporaryFailure, color: 'orange' },
  cli_failure: { label: copy.health.cliFailure, color: 'red' },
  unknown: { label: copy.health.unknown, color: 'grey' },
};

export function HealthTag(props: HealthTagProps) {
  if (props.running > 0) return <Tag color="blue">{copy.health.running(props.running)}</Tag>;
  return <Tag color={healthMap[props.health].color}>{healthMap[props.health].label}</Tag>;
}
