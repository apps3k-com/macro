SELECT EXISTS (
    SELECT 1
    FROM public.comms_channel_participants AS member
    JOIN public.comms_channels AS channel ON channel.id = member.channel_id
    WHERE member.user_id = $2 AND member.left_at IS NULL
      AND (
          -- Setting a channel picture already requires ownership of the uploaded file.
          channel.profile_picture_id = $1::uuid
          OR EXISTS (
              SELECT 1
              FROM public.comms_attachments AS attachment
              JOIN public.comms_messages AS message ON message.id = attachment.message_id
              WHERE message.parent_entity_type = 'channel'
                AND message.parent_entity_id = channel.id::text
                AND message.deleted_at IS NULL
                AND message.sender_id = $3
                AND attachment.entity_type IN ('static/image', 'static/video')
                AND attachment.entity_id = $1::uuid::text
          )
      )
)
