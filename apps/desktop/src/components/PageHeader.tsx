import type { ReactNode } from 'react';
import { Collapse, Typography } from '@douyinfe/semi-ui';

const { Title, Paragraph } = Typography;

export interface PageHeaderProps {
  title: string;
  description: string;
  detailHeader?: string;
  detail?: string;
  action?: ReactNode;
}

export function PageHeader(props: PageHeaderProps) {
  return (
    <header className="page-header">
      <div>
        <Title heading={2}>{props.title}</Title>
        <Paragraph type="tertiary">{props.description}</Paragraph>
        {props.detail && props.detailHeader ? (
          <Collapse>
            <Collapse.Panel header={props.detailHeader} itemKey="more">
              <Paragraph type="tertiary">{props.detail}</Paragraph>
            </Collapse.Panel>
          </Collapse>
        ) : null}
      </div>
      {props.action}
    </header>
  );
}
